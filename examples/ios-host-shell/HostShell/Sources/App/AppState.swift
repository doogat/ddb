import Foundation
import SwiftUI

/// Owns the DoogatDriver and bootstraps all modules.
class AppState: ObservableObject {
    let driver: DoogatDriver

    init() throws {
        let repoPath = Self.repoPath()
        if FileManager.default.fileExists(atPath: repoPath + "/.git") {
            driver = try DoogatDriver(repoPath: repoPath)
        } else {
            driver = try DoogatDriver.createRepo(repoPath: repoPath)
            _ = try driver.registerNode(name: "ios-host-shell")
        }
        try Self.bootstrapModules(driver)
    }

    private static func bootstrapModules(_ driver: DoogatDriver) throws {
        // Order matters: Bookmarks depends on category, Contacts is independent
        try BookmarksModule.bootstrap(driver)
        try ContactsModule.bootstrap(driver)
    }

    private static func repoPath() -> String {
        // Use App Group for widget/extension access, fall back to app support
        if let groupURL = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: "group.com.doogat.ddb"
        ) {
            return groupURL.appendingPathComponent("ddb").path
        }
        let appSupport = FileManager.default.urls(
            for: .applicationSupportDirectory, in: .userDomainMask
        ).first!
        return appSupport.appendingPathComponent("ddb").path
    }
}
