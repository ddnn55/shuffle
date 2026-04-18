import SwiftUI
import UniformTypeIdentifiers

struct ContentView: View {
    @EnvironmentObject private var playerStore: PlayerStore
    @State private var isImporterPresented = false

    var body: some View {
        NavigationStack {
            VStack(spacing: 24) {
                VStack(spacing: 8) {
                    Text(playerStore.currentTitle)
                        .font(.title2.weight(.semibold))
                        .multilineTextAlignment(.center)
                    Text(playerStore.currentSubtitle)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                    Text(playerStore.currentFolderLabel)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity)

                VStack(spacing: 12) {
                    ProgressView(value: playerStore.progress)
                    HStack {
                        Text(playerStore.elapsedText)
                            .font(.caption.monospacedDigit())
                        Spacer()
                        Text(playerStore.durationText)
                            .font(.caption.monospacedDigit())
                    }
                }

                HStack(spacing: 24) {
                    Button {
                        playerStore.previousTrack()
                    } label: {
                        Label("Previous", systemImage: "backward.fill")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.bordered)

                    Button {
                        playerStore.togglePlayPause()
                    } label: {
                        Label(playerStore.isPlaying ? "Pause" : "Play", systemImage: playerStore.isPlaying ? "pause.fill" : "play.fill")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)

                    Button {
                        playerStore.nextTrack()
                    } label: {
                        Label("Next", systemImage: "forward.fill")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.bordered)
                }

                VStack(spacing: 12) {
                    Button(playerStore.hasSelectedFolder ? "Change Folder" : "Choose Folder") {
                        isImporterPresented = true
                    }
                    .buttonStyle(.borderedProminent)

                    if let errorMessage = playerStore.errorMessage {
                        Text(errorMessage)
                            .font(.footnote)
                            .foregroundStyle(.red)
                            .multilineTextAlignment(.center)
                    }

                    if !playerStore.hasSelectedFolder {
                        Text("Pick a folder that contains MP3 files.")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                }

                Spacer()
            }
            .padding(24)
            .navigationTitle("shuffle")
        }
        .fileImporter(
            isPresented: $isImporterPresented,
            allowedContentTypes: [.folder],
            allowsMultipleSelection: false
        ) { result in
            switch result {
            case .success(let urls):
                if let url = urls.first {
                    playerStore.selectFolder(url)
                }
            case .failure(let error):
                playerStore.setError(error.localizedDescription)
            }
        }
    }
}
