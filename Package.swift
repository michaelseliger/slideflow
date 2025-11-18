// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Slideflow",
    platforms: [.macOS(.v15)],
    products: [
        .library(
            name: "Slideflow",
            targets: ["Slideflow"]
        )
    ],
    dependencies: [
        .package(url: "https://github.com/ZipArchive/ZipArchive", from: "2.6.0"),
        .package(url: "https://github.com/CoreOffice/XMLCoder", from: "0.17.0"),
        .package(url: "https://github.com/aspose-slides-cloud/aspose-slides-cloud-swift", from: "24.0.0")
    ],
    targets: [
        .target(
            name: "Slideflow",
            dependencies: [
                "ZipArchive",
                "XMLCoder",
                .product(name: "AsposeSlidesCloud", package: "aspose-slides-cloud-swift")
            ],
            path: "slideflow"
        ),
        .testTarget(
            name: "SlideflowTests",
            dependencies: ["Slideflow"],
            path: "slideflow/SlideflowTests"
        )
    ]
)
