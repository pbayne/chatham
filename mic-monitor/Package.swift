// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "mic-monitor",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "mic-monitor",
            path: "Sources/mic-monitor",
            linkerSettings: [
                // Embed Info.plist into __TEXT,__info_plist so LSUIElement is in
                // effect before AppKit initializes — mirrors audio-capture, so
                // the sidecar never flashes a Dock icon when spawned.
                .unsafeFlags([
                    "-Xlinker", "-sectcreate",
                    "-Xlinker", "__TEXT",
                    "-Xlinker", "__info_plist",
                    "-Xlinker", "Info.plist",
                ])
            ]
        )
    ]
)
