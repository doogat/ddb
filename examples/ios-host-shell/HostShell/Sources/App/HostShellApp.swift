import SwiftUI

@main
struct HostShellApp: App {
    @StateObject private var appState: AppState

    init() {
        do {
            _appState = StateObject(wrappedValue: try AppState())
        } catch {
            fatalError("Failed to initialize Doogat DB: \(error)")
        }
    }

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(appState)
        }
    }
}
