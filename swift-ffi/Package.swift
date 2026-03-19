// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MakoVirtualizationFFI",
    platforms: [.macOS(.v13)],
    products: [
        .library(
            name: "MakoVirtualizationFFI",
            type: .static,
            targets: ["MakoVirtualizationFFI"]
        ),
    ],
    targets: [
        .target(
            name: "MakoVirtualizationFFI",
            path: "Sources/VirtualizationFFI",
            linkerSettings: [
                .linkedFramework("Virtualization"),
                .linkedFramework("vmnet"),
            ]
        ),
    ]
)
