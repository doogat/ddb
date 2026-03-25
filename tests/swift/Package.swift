// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "DoogatDBTests",
    platforms: [.macOS(.v14), .iOS(.v16)],
    targets: [
        .binaryTarget(
            name: "DoogatDBFFI",
            path: "../../out/swift/DoogatDB.xcframework"
        ),
        .target(
            name: "DoogatDB",
            dependencies: ["DoogatDBFFI"],
            path: "Sources/DoogatDB",
            linkerSettings: [
                .linkedLibrary("z"),
                .linkedLibrary("iconv"),
            ]
        ),
        .testTarget(
            name: "DoogatDBTests",
            dependencies: ["DoogatDB"],
            path: "Tests/DoogatDBTests"
        ),
    ]
)
