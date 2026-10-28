// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "CapstanKit",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "CapstanKit", targets: ["CapstanKit"]),
    ],
    targets: [
        .target(name: "CapstanKit"),
        .testTarget(name: "CapstanKitTests", dependencies: ["CapstanKit"]),
    ]
)
