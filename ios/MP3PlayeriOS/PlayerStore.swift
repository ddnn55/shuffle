import AVFoundation
import Combine
import Foundation
import MediaPlayer

@MainActor
final class PlayerStore: NSObject, ObservableObject {
    @Published private(set) var currentTitle = "No folder selected"
    @Published private(set) var currentSubtitle = ""
    @Published private(set) var currentFolderLabel = ""
    @Published private(set) var elapsedText = "00:00"
    @Published private(set) var durationText = "--:--"
    @Published private(set) var progress = 0.0
    @Published private(set) var isPlaying = false
    @Published private(set) var hasSelectedFolder = false
    @Published private(set) var errorMessage: String?

    private let bookmarkKey = "selectedFolderBookmark"
    private let relativeTrackKey = "resumeRelativeTrack"
    private let trackTimeKey = "resumeTrackTime"

    private var folderURL: URL?
    private var trackURLs: [URL] = []
    private var currentIndex = 0
    private var audioPlayer: AVAudioPlayer?
    private var updateTimer: Timer?
    private let commandCenter = MPRemoteCommandCenter.shared()

    override init() {
        super.init()
        configureAudioSession()
        configureRemoteCommands()
        startTimer()
        restoreFolderAndState()
    }

    deinit {
        updateTimer?.invalidate()
    }

    func selectFolder(_ url: URL) {
        errorMessage = nil
        stopAccessingFolder()

        guard url.startAccessingSecurityScopedResource() else {
            setError("Could not access that folder.")
            return
        }

        do {
            let bookmark = try url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
            UserDefaults.standard.set(bookmark, forKey: bookmarkKey)
            try loadFolder(url, restoring: true)
        } catch {
            url.stopAccessingSecurityScopedResource()
            setError(error.localizedDescription)
        }
    }

    func togglePlayPause() {
        guard let audioPlayer else { return }
        if audioPlayer.isPlaying {
            audioPlayer.pause()
            isPlaying = false
        } else {
            audioPlayer.play()
            isPlaying = true
        }
        persistPlaybackState()
        refreshUI()
        updateNowPlayingInfo()
    }

    func nextTrack() {
        guard !trackURLs.isEmpty else { return }
        currentIndex = (currentIndex + 1) % trackURLs.count
        loadCurrentTrack(startAt: 0, autoplay: true)
    }

    func previousTrack() {
        guard !trackURLs.isEmpty else { return }
        if let audioPlayer, audioPlayer.currentTime > 3 {
            audioPlayer.currentTime = 0
            if isPlaying {
                audioPlayer.play()
            }
            persistPlaybackState()
            refreshUI()
            updateNowPlayingInfo()
            return
        }

        currentIndex = (currentIndex - 1 + trackURLs.count) % trackURLs.count
        loadCurrentTrack(startAt: 0, autoplay: true)
    }

    func setError(_ message: String) {
        errorMessage = message
    }

    private func restoreFolderAndState() {
        guard let bookmarkData = UserDefaults.standard.data(forKey: bookmarkKey) else { return }

        var isStale = false
        do {
            let url = try URL(
                resolvingBookmarkData: bookmarkData,
                options: [.withoutUI],
                relativeTo: nil,
                bookmarkDataIsStale: &isStale
            )
            if isStale {
                let refreshedBookmark = try url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
                UserDefaults.standard.set(refreshedBookmark, forKey: bookmarkKey)
            }

            guard url.startAccessingSecurityScopedResource() else {
                setError("Saved folder is no longer accessible.")
                return
            }
            try loadFolder(url, restoring: true)
        } catch {
            setError(error.localizedDescription)
        }
    }

    private func loadFolder(_ url: URL, restoring: Bool) throws {
        let discoveredTracks = try discoverTracks(in: url)
        guard !discoveredTracks.isEmpty else {
            throw PlayerStoreError.noMP3sFound
        }

        folderURL = url
        trackURLs = discoveredTracks
        hasSelectedFolder = true
        currentFolderLabel = url.lastPathComponent

        let savedRelativeTrack = UserDefaults.standard.string(forKey: relativeTrackKey)
        let savedTime = UserDefaults.standard.double(forKey: trackTimeKey)

        if restoring,
           let savedRelativeTrack,
           let restoredIndex = trackURLs.firstIndex(where: { relativePath(for: $0) == savedRelativeTrack }) {
            currentIndex = restoredIndex
            loadCurrentTrack(startAt: savedTime, autoplay: true)
        } else {
            currentIndex = 0
            loadCurrentTrack(startAt: 0, autoplay: false)
        }
    }

    private func discoverTracks(in root: URL) throws -> [URL] {
        let keys: [URLResourceKey] = [.isRegularFileKey]
        guard let enumerator = FileManager.default.enumerator(
            at: root,
            includingPropertiesForKeys: keys,
            options: [.skipsHiddenFiles]
        ) else {
            throw PlayerStoreError.couldNotEnumerateFolder
        }

        var urls: [URL] = []
        for case let fileURL as URL in enumerator {
            guard fileURL.pathExtension.lowercased() == "mp3" else { continue }
            let values = try? fileURL.resourceValues(forKeys: Set(keys))
            if values?.isRegularFile == true {
                urls.append(fileURL)
            }
        }

        return urls.sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
    }

    private func loadCurrentTrack(startAt time: TimeInterval, autoplay: Bool) {
        guard trackURLs.indices.contains(currentIndex) else { return }

        do {
            let player = try AVAudioPlayer(contentsOf: trackURLs[currentIndex])
            player.delegate = self
            player.prepareToPlay()
            player.currentTime = min(max(time, 0), max(player.duration - 0.1, 0))
            audioPlayer = player
            if autoplay {
                player.play()
                isPlaying = true
            } else {
                isPlaying = false
            }
            errorMessage = nil
            persistPlaybackState()
            refreshUI()
            updateNowPlayingInfo()
        } catch {
            setError(error.localizedDescription)
        }
    }

    private func relativePath(for url: URL) -> String {
        guard let folderURL else { return url.lastPathComponent }
        let rootPath = folderURL.standardizedFileURL.path
        let filePath = url.standardizedFileURL.path
        if filePath.hasPrefix(rootPath + "/") {
            return String(filePath.dropFirst(rootPath.count + 1))
        }
        return url.lastPathComponent
    }

    private func persistPlaybackState() {
        guard trackURLs.indices.contains(currentIndex) else { return }
        UserDefaults.standard.set(relativePath(for: trackURLs[currentIndex]), forKey: relativeTrackKey)
        UserDefaults.standard.set(audioPlayer?.currentTime ?? 0, forKey: trackTimeKey)
    }

    private func refreshUI() {
        guard trackURLs.indices.contains(currentIndex) else {
            currentTitle = "No track loaded"
            currentSubtitle = ""
            elapsedText = "00:00"
            durationText = "--:--"
            progress = 0
            return
        }

        let url = trackURLs[currentIndex]
        let metadata = TrackMetadata(url: url)
        currentTitle = metadata.title
        currentSubtitle = metadata.subtitle

        let elapsed = audioPlayer?.currentTime ?? 0
        let duration = audioPlayer?.duration ?? 0
        elapsedText = formatTime(elapsed)
        durationText = duration > 0 ? formatTime(duration) : "--:--"
        progress = duration > 0 ? min(max(elapsed / duration, 0), 1) : 0
    }

    private func startTimer() {
        updateTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in
                guard let self else { return }
                self.refreshUI()
                self.persistPlaybackState()
                self.updateNowPlayingInfo()
            }
        }
    }

    private func configureAudioSession() {
        do {
            try AVAudioSession.sharedInstance().setCategory(.playback, mode: .default)
            try AVAudioSession.sharedInstance().setActive(true)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func configureRemoteCommands() {
        commandCenter.playCommand.isEnabled = true
        commandCenter.pauseCommand.isEnabled = true
        commandCenter.togglePlayPauseCommand.isEnabled = true
        commandCenter.nextTrackCommand.isEnabled = true
        commandCenter.previousTrackCommand.isEnabled = true

        commandCenter.playCommand.addTarget { [weak self] _ in
            Task { @MainActor in
                self?.play()
            }
            return .success
        }

        commandCenter.pauseCommand.addTarget { [weak self] _ in
            Task { @MainActor in
                self?.pause()
            }
            return .success
        }

        commandCenter.togglePlayPauseCommand.addTarget { [weak self] _ in
            Task { @MainActor in
                self?.togglePlayPause()
            }
            return .success
        }

        commandCenter.nextTrackCommand.addTarget { [weak self] _ in
            Task { @MainActor in
                self?.nextTrack()
            }
            return .success
        }

        commandCenter.previousTrackCommand.addTarget { [weak self] _ in
            Task { @MainActor in
                self?.previousTrack()
            }
            return .success
        }
    }

    private func play() {
        guard let audioPlayer, !audioPlayer.isPlaying else { return }
        audioPlayer.play()
        isPlaying = true
        persistPlaybackState()
        refreshUI()
        updateNowPlayingInfo()
    }

    private func pause() {
        guard let audioPlayer, audioPlayer.isPlaying else { return }
        audioPlayer.pause()
        isPlaying = false
        persistPlaybackState()
        refreshUI()
        updateNowPlayingInfo()
    }

    private func updateNowPlayingInfo() {
        guard trackURLs.indices.contains(currentIndex) else {
            MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
            return
        }

        let metadata = TrackMetadata(url: trackURLs[currentIndex])
        var info: [String: Any] = [
            MPMediaItemPropertyTitle: metadata.title,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: audioPlayer?.currentTime ?? 0,
            MPNowPlayingInfoPropertyPlaybackRate: isPlaying ? 1.0 : 0.0
        ]

        if !metadata.artist.isEmpty {
            info[MPMediaItemPropertyArtist] = metadata.artist
        }
        if !metadata.album.isEmpty {
            info[MPMediaItemPropertyAlbumTitle] = metadata.album
        }
        if let duration = audioPlayer?.duration, duration > 0 {
            info[MPMediaItemPropertyPlaybackDuration] = duration
        }

        MPNowPlayingInfoCenter.default().nowPlayingInfo = info
    }

    private func stopAccessingFolder() {
        folderURL?.stopAccessingSecurityScopedResource()
        folderURL = nil
    }

    private func formatTime(_ interval: TimeInterval) -> String {
        let totalSeconds = max(0, Int(interval.rounded(.down)))
        return String(format: "%02d:%02d", totalSeconds / 60, totalSeconds % 60)
    }
}

extension PlayerStore: AVAudioPlayerDelegate {
    nonisolated func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        Task { @MainActor in
            self.nextTrack()
        }
    }
}

private struct TrackMetadata {
    let title: String
    let artist: String
    let album: String

    init(url: URL) {
        let asset = AVAsset(url: url)
        let metadata = asset.commonMetadata

        let titleValue = metadata.firstStringValue(for: .commonIdentifierTitle)
        let artistValue = metadata.firstStringValue(for: .commonIdentifierArtist)
        let albumValue = metadata.firstStringValue(for: .commonIdentifierAlbumName)

        title = titleValue?.isEmpty == false ? titleValue! : url.deletingPathExtension().lastPathComponent
        artist = artistValue ?? ""
        album = albumValue ?? ""
    }

    var subtitle: String {
        switch (artist.isEmpty, album.isEmpty) {
        case (false, false):
            return "\(artist) - \(album)"
        case (false, true):
            return artist
        case (true, false):
            return album
        case (true, true):
            return ""
        }
    }
}

private extension Array where Element == AVMetadataItem {
    func firstStringValue(for identifier: AVMetadataIdentifier) -> String? {
        first(where: { $0.identifier == identifier })?.stringValue
    }
}

enum PlayerStoreError: LocalizedError {
    case noMP3sFound
    case couldNotEnumerateFolder

    var errorDescription: String? {
        switch self {
        case .noMP3sFound:
            return "No MP3 files were found in that folder."
        case .couldNotEnumerateFolder:
            return "Could not read that folder."
        }
    }
}
