// swift-tools-version:5.10

import PackageDescription

let package = Package(
    name: "NativeLogic",
    platforms: [
        .iOS(.v16),
        .macOS(.v14),
    ],
    products: [
        .library(name: "NativeLogic", targets: ["NativeLogic"]),
    ],
    targets: [
        .target(
            name: "NativeLogic",
            path: "Sources/NativeLogic"
        ),
        .testTarget(
            name: "NativeLogicTests",
            dependencies: ["NativeLogic"],
            path: "Tests/NativeLogicTests"
        ),
    ],
    swiftLanguageVersions: [.v5]
)
