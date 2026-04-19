use std::ffi::CString;
use std::sync::mpsc::{self, Receiver, Sender};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{
    NSDate, NSDefaultRunLoopMode, NSMutableDictionary, NSObject, NSRunLoop, NSString,
};
use objc2_media_player::{
    MPMediaItemPropertyAlbumTitle, MPMediaItemPropertyArtist, MPMediaItemPropertyPlaybackDuration,
    MPMediaItemPropertyTitle, MPNowPlayingInfoPropertyElapsedPlaybackTime,
    MPNowPlayingInfoPropertyPlaybackRate,
};

use crate::{ControlAction, Player};

const REMOTE_COMMAND_SUCCESS: isize = 0;
const NOW_PLAYING_STATE_PLAYING: isize = 1;
const NOW_PLAYING_STATE_PAUSED: isize = 2;

#[derive(Clone)]
struct MediaRemoteState {
    playback: PlaybackState,
    elapsed_seconds: f64,
    duration_seconds: Option<f64>,
    title: String,
    artist: String,
    album: String,
}

#[derive(Clone, Copy)]
enum PlaybackState {
    Playing,
    Paused,
}

pub struct MediaRemoteClient {
    app: Retained<NSApplication>,
    now_playing_center: Retained<AnyObject>,
    _command_center: Retained<AnyObject>,
    _handlers: Vec<Retained<AnyObject>>,
    pub actions_rx: Receiver<ControlAction>,
}

impl MediaRemoteClient {
    pub fn sync_state(&self, player: &Player) {
        let details = player.read_current_details();
        let state = MediaRemoteState {
            playback: if player.is_paused() {
                PlaybackState::Paused
            } else {
                PlaybackState::Playing
            },
            elapsed_seconds: player.current_time().as_secs_f64(),
            duration_seconds: player.duration().map(|value| value.as_secs_f64()),
            title: details.title,
            artist: details.artist.unwrap_or_default(),
            album: details.album.unwrap_or_default(),
        };
        update_now_playing(&self.now_playing_center, &state);
    }

    pub fn pump(&self) {
        let run_loop = NSRunLoop::currentRunLoop();
        let limit_date = NSDate::dateWithTimeIntervalSinceNow(0.0);
        let _ = unsafe { run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &limit_date) };
        self.app.updateWindows();
    }
}

impl Drop for MediaRemoteClient {
    fn drop(&mut self) {
        unsafe {
            clear_now_playing(&self.now_playing_center);
        }
    }
}

pub fn start_media_remote_client() -> Option<MediaRemoteClient> {
    let mtm = MainThreadMarker::new()?;
    let (actions_tx, actions_rx) = mpsc::channel::<ControlAction>();
    let app = NSApplication::sharedApplication(mtm);
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    app.finishLaunching();

    let now_playing_center: Retained<AnyObject> =
        unsafe { msg_send![class!(MPNowPlayingInfoCenter), defaultCenter] };
    let command_center: Retained<AnyObject> =
        unsafe { msg_send![class!(MPRemoteCommandCenter), sharedCommandCenter] };
    let handlers = register_command_handlers(&command_center, &actions_tx);

    Some(MediaRemoteClient {
        app,
        now_playing_center,
        _command_center: command_center,
        _handlers: handlers,
        actions_rx,
    })
}

fn register_command_handlers(
    command_center: &AnyObject,
    actions_tx: &Sender<ControlAction>,
) -> Vec<Retained<AnyObject>> {
    let play = unsafe { command_for_selector(command_center, "playCommand") };
    let pause = unsafe { command_for_selector(command_center, "pauseCommand") };
    let toggle = unsafe { command_for_selector(command_center, "togglePlayPauseCommand") };
    let next = unsafe { command_for_selector(command_center, "nextTrackCommand") };
    let previous = unsafe { command_for_selector(command_center, "previousTrackCommand") };

    let play_block = RcBlock::new({
        let actions_tx = actions_tx.clone();
        move |_event: *mut AnyObject| -> isize {
            let _ = actions_tx.send(ControlAction::Play);
            REMOTE_COMMAND_SUCCESS
        }
    });
    let pause_block = RcBlock::new({
        let actions_tx = actions_tx.clone();
        move |_event: *mut AnyObject| -> isize {
            let _ = actions_tx.send(ControlAction::Pause);
            REMOTE_COMMAND_SUCCESS
        }
    });
    let toggle_block = RcBlock::new({
        let actions_tx = actions_tx.clone();
        move |_event: *mut AnyObject| -> isize {
            let _ = actions_tx.send(ControlAction::TogglePause);
            REMOTE_COMMAND_SUCCESS
        }
    });
    let next_block = RcBlock::new({
        let actions_tx = actions_tx.clone();
        move |_event: *mut AnyObject| -> isize {
            let _ = actions_tx.send(ControlAction::Next);
            REMOTE_COMMAND_SUCCESS
        }
    });
    let previous_block = RcBlock::new({
        let actions_tx = actions_tx.clone();
        move |_event: *mut AnyObject| -> isize {
            let _ = actions_tx.send(ControlAction::Previous);
            REMOTE_COMMAND_SUCCESS
        }
    });

    unsafe {
        enable_command(&play);
        enable_command(&pause);
        enable_command(&toggle);
        enable_command(&next);
        enable_command(&previous);
    }

    unsafe {
        vec![
            add_handler(&play, &play_block),
            add_handler(&pause, &pause_block),
            add_handler(&toggle, &toggle_block),
            add_handler(&next, &next_block),
            add_handler(&previous, &previous_block),
        ]
    }
}

unsafe fn command_for_selector(command_center: &AnyObject, selector: &str) -> Retained<AnyObject> {
    match selector {
        "playCommand" => msg_send![command_center, playCommand],
        "pauseCommand" => msg_send![command_center, pauseCommand],
        "togglePlayPauseCommand" => msg_send![command_center, togglePlayPauseCommand],
        "nextTrackCommand" => msg_send![command_center, nextTrackCommand],
        "previousTrackCommand" => msg_send![command_center, previousTrackCommand],
        _ => unreachable!("unsupported selector"),
    }
}

unsafe fn enable_command(command: &AnyObject) {
    let _: () = msg_send![command, setEnabled: true];
}

unsafe fn add_handler(
    command: &AnyObject,
    handler: &RcBlock<dyn Fn(*mut AnyObject) -> isize>,
) -> Retained<AnyObject> {
    msg_send![command, addTargetWithHandler: &**handler]
}

fn update_now_playing(now_playing_center: &AnyObject, state: &MediaRemoteState) {
    let info = NSMutableDictionary::<NSString, NSObject>::new();

    unsafe {
        insert_string(&info, MPMediaItemPropertyTitle, &state.title);
        insert_number(
            &info,
            MPNowPlayingInfoPropertyElapsedPlaybackTime,
            state.elapsed_seconds,
        );
        insert_number(
            &info,
            MPNowPlayingInfoPropertyPlaybackRate,
            match state.playback {
                PlaybackState::Playing => 1.0,
                PlaybackState::Paused => 0.0,
            },
        );
    }

    if let Some(duration_seconds) = state.duration_seconds {
        unsafe {
            insert_number(&info, MPMediaItemPropertyPlaybackDuration, duration_seconds);
        }
    }
    if !state.artist.is_empty() {
        unsafe {
            insert_string(&info, MPMediaItemPropertyArtist, &state.artist);
        }
    }
    if !state.album.is_empty() {
        unsafe {
            insert_string(&info, MPMediaItemPropertyAlbumTitle, &state.album);
        }
    }

    unsafe {
        let _: () = msg_send![now_playing_center, setNowPlayingInfo: Some(&*info)];
        let playback_state = match state.playback {
            PlaybackState::Playing => NOW_PLAYING_STATE_PLAYING,
            PlaybackState::Paused => NOW_PLAYING_STATE_PAUSED,
        };
        let _: () = msg_send![now_playing_center, setPlaybackState: playback_state];
    }
}

unsafe fn clear_now_playing(now_playing_center: &AnyObject) {
    let _: () = msg_send![now_playing_center, setNowPlayingInfo: Option::<&AnyObject>::None];
}

unsafe fn insert_string(
    info: &NSMutableDictionary<NSString, NSObject>,
    key: &'static NSString,
    value: &str,
) {
    let string = unsafe { nsstring_from_str(value) };
    info.insert(key, &string);
}

unsafe fn insert_number(
    info: &NSMutableDictionary<NSString, NSObject>,
    key: &'static NSString,
    value: f64,
) {
    let number: Retained<NSObject> = msg_send![class!(NSNumber), numberWithDouble: value];
    info.insert(key, &number);
}

unsafe fn nsstring_from_str(value: &str) -> Retained<NSString> {
    let sanitized = value.replace('\0', " ");
    let c_string = CString::new(sanitized).expect("interior null bytes should be removed");
    msg_send![class!(NSString), stringWithUTF8String: c_string.as_ptr()]
}
