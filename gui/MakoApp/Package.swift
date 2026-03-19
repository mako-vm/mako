// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MakoApp",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "MakoApp",
            path: "Sources/MakoApp"
        ),
    ]
)
