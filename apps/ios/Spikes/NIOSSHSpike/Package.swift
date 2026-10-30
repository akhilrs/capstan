// swift-tools-version: 6.2
import PackageDescription

// A record of what swift-nio-ssh can do for the iPhone app (package ssh-spike of docs/plans/iphone-app-plan.md).
// Nothing depends on it. swift-nio is named only for NIOPosix, which the loopback server and client need.
let package = Package(
    name: "NIOSSHSpike",
    platforms: [.macOS(.v14), .iOS(.v26)],
    products: [
        .library(name: "NIOSSHSpike", targets: ["NIOSSHSpike"]),
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-nio-ssh.git", exact: "0.15.1"),
        .package(url: "https://github.com/apple/swift-nio.git", from: "2.81.0"),
    ],
    targets: [
        .target(
            name: "NIOSSHSpike",
            dependencies: [
                .product(name: "NIOSSH", package: "swift-nio-ssh"),
                .product(name: "NIOCore", package: "swift-nio"),
                .product(name: "NIOPosix", package: "swift-nio"),
                .product(name: "NIOConcurrencyHelpers", package: "swift-nio"),
            ]),
        .testTarget(
            name: "NIOSSHSpikeTests",
            dependencies: [
                "NIOSSHSpike",
                .product(name: "NIOSSH", package: "swift-nio-ssh"),
                .product(name: "NIOCore", package: "swift-nio"),
                .product(name: "NIOPosix", package: "swift-nio"),
            ]),
    ],
    // The spike records library behaviour; Swift 5 mode keeps NIO's non-Sendable delegates from turning into errors.
    swiftLanguageModes: [.v5]
)
