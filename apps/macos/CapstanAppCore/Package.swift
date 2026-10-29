// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "CapstanAppCore",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "CapstanAppCore", targets: ["CapstanAppCore"]),
    ],
    dependencies: [
        .package(path: "../CapstanKit"),
    ],
    targets: [
        .target(
            name: "CapstanAppCore",
            dependencies: [.product(name: "CapstanKit", package: "CapstanKit")],
            path: "Sources/CapstanAppCore"),
        .testTarget(
            name: "CapstanAppCoreTests",
            dependencies: ["CapstanAppCore", .product(name: "CapstanKit", package: "CapstanKit")]),
    ]
)
