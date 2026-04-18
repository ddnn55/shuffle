import Foundation
import MediaPlayer

final class MediaRemoteBridge {
    private let commandCenter = MPRemoteCommandCenter.shared()
    private let nowPlayingCenter = MPNowPlayingInfoCenter.default()

    init() {
        registerCommands()
    }

    func run() {
        DispatchQueue.global(qos: .utility).async {
            while let line = readLine() {
                self.handle(line: line)
            }
            exit(0)
        }

        RunLoop.main.run()
    }

    private func registerCommands() {
        commandCenter.playCommand.isEnabled = true
        commandCenter.pauseCommand.isEnabled = true
        commandCenter.togglePlayPauseCommand.isEnabled = true
        commandCenter.nextTrackCommand.isEnabled = true
        commandCenter.previousTrackCommand.isEnabled = true

        commandCenter.playCommand.addTarget { [weak self] _ in
            self?.emit("play")
            return .success
        }
        commandCenter.pauseCommand.addTarget { [weak self] _ in
            self?.emit("pause")
            return .success
        }
        commandCenter.togglePlayPauseCommand.addTarget { [weak self] _ in
            self?.emit("toggle")
            return .success
        }
        commandCenter.nextTrackCommand.addTarget { [weak self] _ in
            self?.emit("next")
            return .success
        }
        commandCenter.previousTrackCommand.addTarget { [weak self] _ in
            self?.emit("previous")
            return .success
        }
    }

    private func emit(_ action: String) {
        FileHandle.standardOutput.write(Data((action + "\n").utf8))
    }

    private func handle(line: String) {
        if line == "QUIT" {
            exit(0)
        }

        let parts = line.split(separator: "\t", omittingEmptySubsequences: false)
        guard parts.count >= 7, parts[0] == "STATE" else {
            return
        }

        let playback = String(parts[1])
        let elapsed = Double(parts[2]) ?? 0
        let duration = Double(parts[3]) ?? -1
        let title = String(parts[4])
        let artist = String(parts[5])
        let album = String(parts[6])

        var info: [String: Any] = [
            MPMediaItemPropertyTitle: title,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: elapsed,
            MPNowPlayingInfoPropertyPlaybackRate: playback == "playing" ? 1.0 : 0.0,
        ]

        if !artist.isEmpty {
            info[MPMediaItemPropertyArtist] = artist
        }
        if !album.isEmpty {
            info[MPMediaItemPropertyAlbumTitle] = album
        }
        if duration >= 0 {
            info[MPMediaItemPropertyPlaybackDuration] = duration
        }

        nowPlayingCenter.nowPlayingInfo = info
        if #available(macOS 10.12.2, *) {
            nowPlayingCenter.playbackState = playback == "playing" ? .playing : .paused
        }
    }
}

MediaRemoteBridge().run()
