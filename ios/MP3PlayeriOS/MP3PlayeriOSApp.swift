import SwiftUI

@main
struct MP3PlayeriOSApp: App {
    @StateObject private var playerStore = PlayerStore()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(playerStore)
        }
    }
}
