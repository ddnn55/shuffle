use std::env;
use std::fs;
use std::fs::File;
use std::io::{BufReader, Stdout, Write, stderr, stdout};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::style::Print;
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use id3::{Tag, TagLike};
use rand::seq::SliceRandom;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

const STATE_FILE_NAME: &str = "state";

#[derive(Debug)]
enum PlayerError {
    NoFilesFound,
    InvalidPath(String),
    Usage(String),
    TerminalSetup(String),
    Playback(String),
}

impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoFilesFound => write!(f, "No mp3 files found."),
            Self::InvalidPath(path) => write!(f, "Path not found or unsupported: {path}"),
            Self::Usage(message) => write!(f, "{message}"),
            Self::TerminalSetup(message) => write!(f, "Terminal setup failed: {message}"),
            Self::Playback(message) => write!(f, "Playback failed: {message}"),
        }
    }
}

impl std::error::Error for PlayerError {}

#[derive(Clone, Copy)]
struct Rect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

impl Rect {
    fn contains(&self, column: u16, row: u16) -> bool {
        column >= self.x
            && column < self.x.saturating_add(self.width)
            && row >= self.y
            && row < self.y.saturating_add(self.height)
    }
}

#[derive(Clone, Copy)]
struct RenderLayout {
    previous_button: Rect,
    next_button: Rect,
}

struct TerminalGuard {
    stdout: Stdout,
    active: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self, PlayerError> {
        enable_raw_mode().map_err(|error| PlayerError::TerminalSetup(error.to_string()))?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide)
            .map_err(|error| PlayerError::TerminalSetup(error.to_string()))?;
        Ok(Self {
            stdout,
            active: true,
        })
    }

    fn stdout(&mut self) -> &mut Stdout {
        &mut self.stdout
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        let _ = disable_raw_mode();
        let _ = execute!(
            self.stdout,
            Show,
            DisableMouseCapture,
            LeaveAlternateScreen,
            Clear(ClearType::All)
        );
        let _ = self.stdout.flush();
    }
}

#[derive(Clone)]
struct PersistedState {
    track_path: PathBuf,
    position_seconds: f64,
}

#[derive(Clone)]
struct TrackDetails {
    title: String,
    artist: Option<String>,
    album: Option<String>,
    created_date: Option<String>,
}

struct ScanProgress {
    files_found: usize,
    directories_scanned: usize,
    last_draw: Instant,
}

impl ScanProgress {
    fn new() -> Self {
        Self {
            files_found: 0,
            directories_scanned: 0,
            last_draw: Instant::now() - Duration::from_secs(1),
        }
    }

    fn visit_directory(&mut self, path: &Path) {
        self.directories_scanned += 1;
        self.draw(path, false);
    }

    fn found_file(&mut self, path: &Path) {
        self.files_found += 1;
        self.draw(path, true);
    }

    fn finish(&self) {
        let mut output = stderr();
        let _ = write!(output, "\r\x1b[2K");
        let _ = output.flush();
    }

    fn draw(&mut self, path: &Path, force: bool) {
        if !force && self.last_draw.elapsed() < Duration::from_millis(50) {
            return;
        }

        let mut output = stderr();
        let location = truncate_middle(&path.display().to_string(), 72);
        let _ = write!(
            output,
            "\r\x1b[2KScanning MP3s: {} files in {} folders  {}",
            self.files_found, self.directories_scanned, location
        );
        let _ = output.flush();
        self.last_draw = Instant::now();
    }
}

#[derive(Clone, Copy)]
enum ControlAction {
    Play,
    Pause,
    TogglePause,
    Previous,
    Next,
}

struct MediaRemoteClient {
    updates_tx: Sender<String>,
    actions_rx: Receiver<ControlAction>,
    child: Child,
}

impl MediaRemoteClient {
    fn sync_state(&self, player: &mut Player) {
        let details = player.read_current_details();
        let line = format!(
            "STATE\t{}\t{:.3}\t{:.3}\t{}\t{}\t{}\n",
            if player.is_paused() {
                "paused"
            } else {
                "playing"
            },
            player.current_time().as_secs_f64(),
            player
                .duration()
                .map(|value| value.as_secs_f64())
                .unwrap_or(-1.0),
            sanitize_remote_field(&details.title),
            sanitize_remote_field(details.artist.as_deref().unwrap_or("")),
            sanitize_remote_field(details.album.as_deref().unwrap_or("")),
        );
        let _ = self.updates_tx.send(line);
    }
}

impl Drop for MediaRemoteClient {
    fn drop(&mut self) {
        let _ = self.updates_tx.send("QUIT\n".to_string());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Player {
    files: Vec<PathBuf>,
    order: Vec<usize>,
    index: usize,
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    sink: Sink,
    started_at: Option<Instant>,
    current_offset: Duration,
    current_duration: Option<Duration>,
    paused: bool,
}

impl Player {
    fn new(files: Vec<PathBuf>) -> Result<Self, PlayerError> {
        let mut order: Vec<usize> = (0..files.len()).collect();
        shuffle(&mut order);

        let (stream, stream_handle) = OutputStream::try_default()
            .map_err(|error| PlayerError::Playback(error.to_string()))?;
        let sink = Sink::try_new(&stream_handle)
            .map_err(|error| PlayerError::Playback(error.to_string()))?;

        Ok(Self {
            files,
            order,
            index: 0,
            _stream: stream,
            stream_handle,
            sink,
            started_at: None,
            current_offset: Duration::ZERO,
            current_duration: None,
            paused: false,
        })
    }

    fn start(&mut self) -> Result<(), PlayerError> {
        self.load_current_track(Duration::ZERO)
    }

    fn restore(&mut self, state: &PersistedState) -> Result<bool, PlayerError> {
        let Some(file_index) = self.files.iter().position(|path| path == &state.track_path) else {
            return Ok(false);
        };

        if let Some(order_index) = self
            .order
            .iter()
            .position(|candidate| *candidate == file_index)
        {
            self.order.swap(self.index, order_index);
        }

        let offset = Duration::from_secs_f64(state.position_seconds.max(0.0));
        self.load_current_track(offset)?;
        Ok(true)
    }

    fn next_track(&mut self) -> Result<(), PlayerError> {
        self.advance(1);
        self.load_current_track(Duration::ZERO)
    }

    fn previous_or_restart(&mut self) -> Result<(), PlayerError> {
        if self.current_time().as_secs_f64() > 3.0 {
            return self.restart_current_track();
        }

        self.advance(-1);
        self.load_current_track(Duration::ZERO)
    }

    fn restart_current_track(&mut self) -> Result<(), PlayerError> {
        self.load_current_track(Duration::ZERO)
    }

    fn has_finished_playback(&self) -> bool {
        !self.paused && self.sink.empty()
    }

    fn shutdown(&mut self) {
        self.sink.stop();
        self.started_at = None;
    }

    fn toggle_pause(&mut self) {
        if self.paused {
            self.play();
        } else {
            self.pause();
        }
    }

    fn play(&mut self) {
        if !self.paused {
            return;
        }

        self.sink.play();
        self.started_at = Some(Instant::now());
        self.paused = false;
    }

    fn pause(&mut self) {
        if self.paused {
            return;
        }

        self.current_offset = self.current_time();
        self.sink.pause();
        self.started_at = None;
        self.paused = true;
    }

    fn current_file(&self) -> &Path {
        &self.files[self.order[self.index]]
    }

    fn current_time(&self) -> Duration {
        match self.started_at {
            Some(started_at) => {
                let position = self.current_offset.saturating_add(started_at.elapsed());
                match self.current_duration {
                    Some(duration) => position.min(duration),
                    None => position,
                }
            }
            None => self.current_offset,
        }
    }

    fn duration(&self) -> Option<Duration> {
        self.current_duration
    }

    fn is_paused(&self) -> bool {
        self.paused
    }

    fn persisted_state(&self) -> PersistedState {
        PersistedState {
            track_path: self.current_file().to_path_buf(),
            position_seconds: self.current_time().as_secs_f64(),
        }
    }

    fn read_current_details(&self) -> TrackDetails {
        read_track_details(self.current_file())
    }

    fn total_tracks(&self) -> usize {
        self.files.len()
    }

    fn advance(&mut self, step: isize) {
        let next_index = self.index as isize + step;
        if next_index >= self.order.len() as isize {
            shuffle(&mut self.order);
            self.index = 0;
            return;
        }

        if next_index < 0 {
            self.index = self.order.len().saturating_sub(1);
            return;
        }

        self.index = next_index as usize;
    }

    fn load_current_track(&mut self, offset: Duration) -> Result<(), PlayerError> {
        let file = File::open(self.current_file()).map_err(|error| {
            PlayerError::Playback(format!(
                "Could not open {} ({error})",
                self.current_file().display()
            ))
        })?;

        let decoder = Decoder::new(BufReader::new(file))
            .map_err(|error| PlayerError::Playback(error.to_string()))?;
        let duration = decoder
            .total_duration()
            .or_else(|| afinfo_duration(self.current_file()));

        let sink = Sink::try_new(&self.stream_handle)
            .map_err(|error| PlayerError::Playback(error.to_string()))?;
        sink.append(decoder);
        if !offset.is_zero() {
            sink.try_seek(offset)
                .map_err(|error| PlayerError::Playback(error.to_string()))?;
        }
        sink.play();

        self.sink.stop();
        self.sink = sink;
        self.started_at = Some(Instant::now());
        self.current_offset = offset;
        self.current_duration = duration;
        self.paused = false;
        Ok(())
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), PlayerError> {
    let mut files = resolve_input_files()?;
    files.sort_by(|left, right| left.to_string_lossy().cmp(&right.to_string_lossy()));
    if files.is_empty() {
        return Err(PlayerError::NoFilesFound);
    }

    let persisted_state = load_persisted_state();
    let mut terminal = TerminalGuard::enter()?;

    render_startup_status(
        terminal.stdout(),
        &[
            "shuffle",
            "",
            &format!("Found {} mp3s", files.len()),
            "Starting audio output...",
        ],
    )?;
    let mut player = Player::new(files)?;

    render_startup_status(
        terminal.stdout(),
        &[
            "shuffle",
            "",
            &format!("Found {} mp3s", player.total_tracks()),
            "Connecting system media controls...",
        ],
    )?;
    let media_remote = start_media_remote_client();

    render_startup_status(
        terminal.stdout(),
        &[
            "shuffle",
            "",
            &format!("Found {} mp3s", player.total_tracks()),
            if persisted_state.is_some() {
                "Restoring previous track..."
            } else {
                "Loading first track..."
            },
        ],
    )?;

    let resumed = match persisted_state.as_ref() {
        Some(state) => player.restore(state)?,
        None => false,
    };
    if !resumed {
        player.start()?;
    }

    save_persisted_state(&player.persisted_state());
    if let Some(client) = media_remote.as_ref() {
        client.sync_state(&mut player);
    }
    let mut last_state_save = Instant::now();
    let mut layout = render(&mut terminal, &mut player)?;

    loop {
        while let Some(action) = media_remote
            .as_ref()
            .and_then(|client| client.actions_rx.try_recv().ok())
        {
            apply_control_action(&mut player, action)?;
            save_persisted_state(&player.persisted_state());
            if let Some(client) = media_remote.as_ref() {
                client.sync_state(&mut player);
            }
            last_state_save = Instant::now();
        }

        while event::poll(Duration::from_millis(100))
            .map_err(|error| PlayerError::TerminalSetup(error.to_string()))?
        {
            match event::read().map_err(|error| PlayerError::TerminalSetup(error.to_string()))? {
                Event::Key(key) => match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        save_persisted_state(&player.persisted_state());
                        if let Some(client) = media_remote.as_ref() {
                            client.sync_state(&mut player);
                        }
                        player.shutdown();
                        return Ok(());
                    }
                    KeyCode::Char(' ') => {
                        apply_control_action(&mut player, ControlAction::TogglePause)?;
                        save_persisted_state(&player.persisted_state());
                        if let Some(client) = media_remote.as_ref() {
                            client.sync_state(&mut player);
                        }
                        last_state_save = Instant::now();
                    }
                    KeyCode::Left | KeyCode::Char('h') => {
                        apply_control_action(&mut player, ControlAction::Previous)?;
                        save_persisted_state(&player.persisted_state());
                        if let Some(client) = media_remote.as_ref() {
                            client.sync_state(&mut player);
                        }
                        last_state_save = Instant::now();
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        apply_control_action(&mut player, ControlAction::Next)?;
                        save_persisted_state(&player.persisted_state());
                        if let Some(client) = media_remote.as_ref() {
                            client.sync_state(&mut player);
                        }
                        last_state_save = Instant::now();
                    }
                    _ => {}
                },
                Event::Mouse(mouse) => {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if layout.previous_button.contains(mouse.column, mouse.row) {
                            apply_control_action(&mut player, ControlAction::Previous)?;
                            save_persisted_state(&player.persisted_state());
                            if let Some(client) = media_remote.as_ref() {
                                client.sync_state(&mut player);
                            }
                            last_state_save = Instant::now();
                        } else if layout.next_button.contains(mouse.column, mouse.row) {
                            apply_control_action(&mut player, ControlAction::Next)?;
                            save_persisted_state(&player.persisted_state());
                            if let Some(client) = media_remote.as_ref() {
                                client.sync_state(&mut player);
                            }
                            last_state_save = Instant::now();
                        }
                    }
                }
                Event::Resize(_, _) => {}
                Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
            }
        }

        if player.has_finished_playback() {
            apply_control_action(&mut player, ControlAction::Next)?;
            save_persisted_state(&player.persisted_state());
            if let Some(client) = media_remote.as_ref() {
                client.sync_state(&mut player);
            }
            last_state_save = Instant::now();
        } else if last_state_save.elapsed() >= Duration::from_secs(1) {
            save_persisted_state(&player.persisted_state());
            if let Some(client) = media_remote.as_ref() {
                client.sync_state(&mut player);
            }
            last_state_save = Instant::now();
        }

        layout = render(&mut terminal, &mut player)?;
        thread::sleep(Duration::from_millis(16));
    }
}

fn apply_control_action(player: &mut Player, action: ControlAction) -> Result<(), PlayerError> {
    match action {
        ControlAction::Play => {
            player.play();
            Ok(())
        }
        ControlAction::Pause => {
            player.pause();
            Ok(())
        }
        ControlAction::TogglePause => {
            player.toggle_pause();
            Ok(())
        }
        ControlAction::Previous => player.previous_or_restart(),
        ControlAction::Next => player.next_track(),
    }
}

#[cfg(target_os = "macos")]
fn start_media_remote_client() -> Option<MediaRemoteClient> {
    let helper_path = media_remote_helper_path()?;
    let mut child = Command::new(helper_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;

    let child_stdin = child.stdin.take()?;
    let child_stdout = child.stdout.take()?;

    let (updates_tx, updates_rx) = mpsc::channel::<String>();
    let (actions_tx, actions_rx) = mpsc::channel::<ControlAction>();

    thread::spawn(move || {
        let mut writer = child_stdin;
        while let Ok(message) = updates_rx.recv() {
            if writer.write_all(message.as_bytes()).is_err() {
                break;
            }
            if writer.flush().is_err() {
                break;
            }
        }
    });

    thread::spawn(move || {
        let mut reader = std::io::BufReader::new(child_stdout);
        let mut line = String::new();
        loop {
            line.clear();
            let Ok(bytes_read) = std::io::BufRead::read_line(&mut reader, &mut line) else {
                break;
            };
            if bytes_read == 0 {
                break;
            }
            let action = match line.trim() {
                "play" => Some(ControlAction::Play),
                "pause" => Some(ControlAction::Pause),
                "toggle" => Some(ControlAction::TogglePause),
                "next" => Some(ControlAction::Next),
                "previous" => Some(ControlAction::Previous),
                _ => None,
            };
            if let Some(action) = action {
                let _ = actions_tx.send(action);
            }
        }
    });

    Some(MediaRemoteClient {
        updates_tx,
        actions_rx,
        child,
    })
}

#[cfg(not(target_os = "macos"))]
fn start_media_remote_client() -> Option<MediaRemoteClient> {
    None
}

#[cfg(target_os = "macos")]
fn media_remote_helper_path() -> Option<PathBuf> {
    if let Some(packaged_path) = packaged_media_remote_helper_path() {
        if packaged_path.exists() {
            return Some(packaged_path);
        }
    }

    let build_path = PathBuf::from(option_env!("SHUFFLE_MEDIA_HELPER")?);
    build_path.exists().then_some(build_path)
}

#[cfg(target_os = "macos")]
fn packaged_media_remote_helper_path() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?;
    let bin_dir = executable.parent()?;
    let prefix_dir = bin_dir.parent()?;
    Some(prefix_dir.join("libexec").join("media_remote_helper"))
}

fn sanitize_remote_field(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if matches!(character, '\t' | '\n' | '\r') {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn resolve_input_files() -> Result<Vec<PathBuf>, PlayerError> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.len() > 1 {
        return Err(PlayerError::Usage(
            "Usage: shuffle [path-to-folder-or-file.mp3]".to_string(),
        ));
    }

    if let Some(argument) = arguments.first() {
        let input = absolute_path(argument)?;
        if input.is_dir() {
            let files = find_mp3_files(&input);
            return Ok(files);
        }

        if is_mp3_file(&input) {
            return Ok(vec![input]);
        }

        return Err(PlayerError::InvalidPath(argument.clone()));
    }

    let current_dir =
        env::current_dir().map_err(|error| PlayerError::InvalidPath(error.to_string()))?;
    Ok(find_mp3_files(&current_dir))
}

fn absolute_path(input: &str) -> Result<PathBuf, PlayerError> {
    let path = PathBuf::from(input);
    let absolute = if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .map_err(|error| PlayerError::InvalidPath(error.to_string()))?
            .join(path)
    };

    if absolute.exists() {
        Ok(absolute)
    } else {
        Err(PlayerError::InvalidPath(input.to_string()))
    }
}

fn find_mp3_files(root: &Path) -> Vec<PathBuf> {
    let mut progress = ScanProgress::new();
    let mut files = Vec::new();
    visit_directory(root, &mut files, &mut progress);
    progress.finish();
    files
}

fn visit_directory(path: &Path, files: &mut Vec<PathBuf>, progress: &mut ScanProgress) {
    progress.visit_directory(path);
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };

    for entry in entries.flatten() {
        let child = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };

        if metadata.is_dir() {
            visit_directory(&child, files, progress);
            continue;
        }

        if metadata.is_file() && is_mp3_file(&child) {
            files.push(child);
            progress.found_file(files.last().expect("just pushed file"));
        }
    }
}

fn is_mp3_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("mp3"))
        .unwrap_or(false)
}

fn shuffle(values: &mut [usize]) {
    if values.len() < 2 {
        return;
    }

    values.shuffle(&mut rand::thread_rng());
}

fn afinfo_duration(path: &Path) -> Option<Duration> {
    let output = Command::new("afinfo").arg(path).output().ok()?;
    parse_afinfo_duration(&String::from_utf8_lossy(&output.stdout))
        .or_else(|| parse_afinfo_duration(&String::from_utf8_lossy(&output.stderr)))
        .map(Duration::from_secs_f64)
}

fn parse_afinfo_duration(text: &str) -> Option<f64> {
    for marker in ["estimated duration:", "duration:"] {
        if let Some(index) = text.find(marker) {
            let remainder = &text[index + marker.len()..];
            let seconds: String = remainder
                .trim_start()
                .chars()
                .take_while(|character| character.is_ascii_digit() || *character == '.')
                .collect();
            if let Ok(value) = seconds.parse::<f64>() {
                return Some(value);
            }
        }
    }
    None
}

fn state_directory() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".shuffle"))
}

fn state_file_path() -> Option<PathBuf> {
    state_directory().map(|directory| directory.join(STATE_FILE_NAME))
}

fn load_persisted_state() -> Option<PersistedState> {
    let path = state_file_path()?;
    let contents = fs::read_to_string(path).ok()?;
    let mut track_path = None;
    let mut position_seconds = None;

    for line in contents.lines() {
        let (key, value) = line.split_once('=')?;
        match key {
            "track" => track_path = Some(PathBuf::from(value)),
            "position" => position_seconds = value.parse::<f64>().ok(),
            _ => {}
        }
    }

    Some(PersistedState {
        track_path: track_path?,
        position_seconds: position_seconds?,
    })
}

fn save_persisted_state(state: &PersistedState) {
    let Some(directory) = state_directory() else {
        return;
    };
    let Some(path) = state_file_path() else {
        return;
    };

    if fs::create_dir_all(&directory).is_err() {
        return;
    }

    let contents = format!(
        "track={}\nposition={:.3}\n",
        state.track_path.display(),
        state.position_seconds
    );
    let temp_path = directory.join(format!("{STATE_FILE_NAME}.tmp"));
    if fs::write(&temp_path, contents).is_ok() {
        let _ = fs::rename(temp_path, path);
    }
}

fn read_track_details(path: &Path) -> TrackDetails {
    let fallback_title = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Unknown Track")
        .to_string();

    let Ok(tag) = Tag::read_from_path(path) else {
        return TrackDetails {
            title: fallback_title,
            artist: None,
            album: None,
            created_date: filesystem_created_date(path),
        };
    };

    TrackDetails {
        title: tag.title().unwrap_or(&fallback_title).to_string(),
        artist: tag
            .artist()
            .map(str::to_string)
            .filter(|value| !value.is_empty()),
        album: tag
            .album()
            .map(str::to_string)
            .filter(|value| !value.is_empty()),
        created_date: filesystem_created_date(path),
    }
}

fn filesystem_created_date(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    let created = metadata.created().ok()?;
    let created_at: DateTime<Local> = created.into();
    Some(created_at.format("%Y-%m-%d").to_string())
}

fn render(terminal: &mut TerminalGuard, player: &mut Player) -> Result<RenderLayout, PlayerError> {
    let (width, height) =
        terminal::size().map_err(|error| PlayerError::TerminalSetup(error.to_string()))?;
    let width = width.saturating_sub(1).max(1) as usize;
    let height = height.max(1) as usize;
    let mut canvas = vec![vec![' '; width]; height];

    if width < 40 || height < 18 {
        let message = [
            "shuffle",
            "Resize terminal to at least 40x18",
            "q or esc quits",
        ];
        let start_row = height.saturating_div(2).saturating_sub(1);
        for (offset, line) in message.iter().enumerate() {
            draw_text(&mut canvas, start_row + offset, 0, &centered(line, width));
        }
        flush_canvas(terminal.stdout(), &canvas)?;
        return Ok(RenderLayout {
            previous_button: Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            next_button: Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
        });
    }

    let details = player.read_current_details();
    let subtitle = match (&details.artist, &details.album) {
        (Some(artist), Some(album)) => format!("{artist} - {album}"),
        (Some(artist), None) => artist.clone(),
        (None, Some(album)) => album.clone(),
        (None, None) => player
            .current_file()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Unknown Track")
            .to_string(),
    };
    let created_label = details
        .created_date
        .as_ref()
        .map(|date| format!("File created {date}"))
        .unwrap_or_else(|| "File created unknown".to_string());
    let library_label = format!("{} mp3s", format_count(player.total_tracks()));
    let duration = player.duration();
    let current = player.current_time();
    let time_text = format!(
        "{} / {}",
        format_time(current),
        duration
            .map(format_time)
            .unwrap_or_else(|| "--:--".to_string())
    );

    let content_width = width.min(96).max(40);
    let title_width = content_width.min(width);
    let subtitle_width = content_width.min(width);
    let meta_width = content_width.min(width);
    let timeline_width = content_width.min(width.saturating_sub(4)).max(24);
    let prev_top = "╭──────────╮";
    let prev_marker = "│ ◀  PREV  │";
    let prev_bottom = "╰──────────╯";
    let next_top = "╭──────────╮";
    let next_marker = "│  NEXT  ▶ │";
    let next_bottom = "╰──────────╯";
    let button_gap = 2usize;
    let progress_width = timeline_width
        .saturating_sub(display_width(prev_marker) + display_width(next_marker) + button_gap * 2)
        .max(16);

    let title_lines = split_for_width(&details.title, title_width);
    let subtitle_line = truncate_middle(&subtitle, subtitle_width);
    let meta_line = truncate_middle(&created_label, meta_width);
    let timeline_bar = progress_bar(current, duration, progress_width);
    let control_middle_line = format!(
        "{prev_marker}{}{timeline_bar}{}{next_marker}",
        " ".repeat(button_gap),
        " ".repeat(button_gap)
    );
    let control_top_line = format!(
        "{}{}{}{}",
        prev_top,
        " ".repeat(button_gap),
        " ".repeat(progress_width),
        format!("{}{}", " ".repeat(button_gap), next_top)
    );
    let control_bottom_line = format!(
        "{}{}{}{}",
        prev_bottom,
        " ".repeat(button_gap),
        " ".repeat(progress_width),
        format!("{}{}", " ".repeat(button_gap), next_bottom)
    );
    let time_line = time_text;

    let mut lines = Vec::new();
    lines.extend(title_lines.into_iter().take(3));
    lines.push(String::new());
    lines.push(subtitle_line);
    lines.push(String::new());
    lines.push(meta_line);
    lines.push(String::new());
    lines.push(String::new());
    lines.push(control_top_line.clone());
    lines.push(control_middle_line.clone());
    lines.push(control_bottom_line.clone());
    lines.push(time_line);

    let block_height = lines.len();
    let start_row = height.saturating_sub(block_height) / 2;
    for (offset, line) in lines.iter().enumerate() {
        draw_text(&mut canvas, start_row + offset, 0, &centered(line, width));
    }
    let library_column = width.saturating_sub(library_label.chars().count());
    draw_text(&mut canvas, 0, library_column, &library_label);

    let timeline_row = start_row + block_height.saturating_sub(4);
    let centered_timeline = centered(&control_middle_line, width);
    let line_start = centered_timeline
        .chars()
        .position(|character| character != ' ')
        .unwrap_or(0);
    let prev_offset = char_index_of(&centered_timeline, prev_marker).unwrap_or(line_start);
    let next_offset = char_index_of(&centered_timeline, next_marker).unwrap_or(line_start);

    let prev_rect = Rect {
        x: prev_offset as u16,
        y: timeline_row.saturating_sub(1) as u16,
        width: display_width(prev_marker) as u16,
        height: 3,
    };
    let next_rect = Rect {
        x: next_offset as u16,
        y: timeline_row.saturating_sub(1) as u16,
        width: display_width(next_marker) as u16,
        height: 3,
    };

    flush_canvas(terminal.stdout(), &canvas)?;
    Ok(RenderLayout {
        previous_button: prev_rect,
        next_button: next_rect,
    })
}

fn flush_canvas(stdout: &mut Stdout, canvas: &[Vec<char>]) -> Result<(), PlayerError> {
    execute!(stdout, MoveTo(0, 0), Clear(ClearType::All))
        .map_err(|error| PlayerError::TerminalSetup(error.to_string()))?;
    for (row_index, row) in canvas.iter().enumerate() {
        let line = row.iter().collect::<String>();
        execute!(stdout, MoveTo(0, row_index as u16), Print(&line))
            .map_err(|error| PlayerError::TerminalSetup(error.to_string()))?;
    }
    stdout
        .flush()
        .map_err(|error| PlayerError::TerminalSetup(error.to_string()))?;
    Ok(())
}

fn render_startup_status(stdout: &mut Stdout, lines: &[&str]) -> Result<(), PlayerError> {
    let (width, height) =
        terminal::size().map_err(|error| PlayerError::TerminalSetup(error.to_string()))?;
    let width = width.saturating_sub(1).max(1) as usize;
    let height = height.max(1) as usize;
    let mut canvas = vec![vec![' '; width]; height];
    let start_row = height.saturating_sub(lines.len()) / 2;

    for (offset, line) in lines.iter().enumerate() {
        draw_text(
            &mut canvas,
            start_row + offset,
            0,
            &centered(&truncate_middle(line, width), width),
        );
    }

    flush_canvas(stdout, &canvas)
}

fn draw_text(canvas: &mut [Vec<char>], row: usize, column: usize, text: &str) {
    if row >= canvas.len() || column >= canvas[row].len() {
        return;
    }

    for (offset, character) in text.chars().enumerate() {
        let target = column + offset;
        if target >= canvas[row].len() {
            break;
        }
        canvas[row][target] = character;
    }
}

fn truncate_middle(text: &str, max_length: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_length || max_length <= 3 {
        return text.to_string();
    }

    let keep = (max_length - 3) / 2;
    let prefix: String = text.chars().take(keep).collect();
    let suffix: String = text
        .chars()
        .rev()
        .take(max_length - 3 - keep)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{prefix}...{suffix}")
}

fn centered(text: &str, width: usize) -> String {
    let content: String = text.chars().take(width).collect();
    let padding = width.saturating_sub(display_width(&content));
    let left = padding / 2;
    let right = padding - left;
    format!("{}{}{}", " ".repeat(left), content, " ".repeat(right))
}

fn display_width(text: &str) -> usize {
    text.chars().count()
}

fn char_index_of(haystack: &str, needle: &str) -> Option<usize> {
    let byte_index = haystack.find(needle)?;
    Some(haystack[..byte_index].chars().count())
}

fn split_for_width(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    if text.chars().count() <= width {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };

        if candidate.chars().count() <= width {
            current = candidate;
            continue;
        }

        if !current.is_empty() {
            lines.push(current);
        }

        if word.chars().count() <= width {
            current = word.to_string();
            continue;
        }

        let mut chunk = String::new();
        for character in word.chars() {
            chunk.push(character);
            if chunk.chars().count() == width.saturating_sub(1) {
                lines.push(format!("{chunk}."));
                chunk.clear();
            }
        }
        current = chunk;
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}

fn progress_bar(current: Duration, duration: Option<Duration>, width: usize) -> String {
    let usable_width = width.max(10);
    let inner_width = usable_width.saturating_sub(2);

    let Some(duration) = duration else {
        return format!("[{}]", " ".repeat(inner_width));
    };

    let duration_seconds = duration.as_secs_f64();
    if duration_seconds <= 0.0 {
        return format!("[{}]", " ".repeat(inner_width));
    }

    let ratio = (current.as_secs_f64() / duration_seconds).clamp(0.0, 1.0);
    let total_units = inner_width.saturating_mul(8);
    let filled_units = ((total_units as f64) * ratio).round() as usize;
    let full_blocks = (filled_units / 8).min(inner_width);
    let partial_index = if full_blocks < inner_width {
        filled_units % 8
    } else {
        0
    };
    let partial_block = match partial_index {
        1 => "▏",
        2 => "▎",
        3 => "▍",
        4 => "▌",
        5 => "▋",
        6 => "▊",
        7 => "▉",
        _ => "",
    };
    let used_cells = full_blocks + usize::from(!partial_block.is_empty());
    let empty_cells = inner_width.saturating_sub(used_cells);
    format!(
        "[{}{}{}]",
        "█".repeat(full_blocks),
        partial_block,
        " ".repeat(empty_cells)
    )
}

fn format_time(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn format_count(value: usize) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, character) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(character);
    }

    formatted.chars().rev().collect()
}
