//
//  PowerPointParser.swift
//  Slideflow
//
//  Parses PPTX files using ZipArchive + XMLCoder
//

import Foundation
// import ZipArchive  // TODO: Link via SPM
// import XMLCoder    // TODO: Link via SPM

// Native unzip implementation using macOS command-line tool
struct NativeUnzipper {
    static func unzipFile(atPath path: String, toDestination destination: String) -> Bool {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/unzip")
        process.arguments = ["-q", "-d", destination, path]

        do {
            try process.run()
            process.waitUntilExit()
            return process.terminationStatus == 0
        } catch {
            print("⚠️ Unzip failed: \(error.localizedDescription)")
            return false
        }
    }
}

enum PowerPointParserError: Error {
    case fileNotFound
    case invalidFormat
    case extractionFailed(underlying: Error)
    case parsingFailed(underlying: Error)
}

struct PresentationMetadata {
    let slideCount: Int
    let filename: String
    let filePath: String
}

struct SlideData {
    let slideNumber: Int16
    let textContent: String
    let thumbnailPath: String
    let hasImages: Bool
    let hasMedia: Bool
    let slideLayout: String?
}

class PowerPointParser {
    private let fileManager = FileManager.default
    // private let xmlDecoder = XMLDecoder()  // TODO: Enable when XMLCoder is linked

    init() {}

    /// Extract PPTX ZIP archive to temporary location
    func extractArchive(at path: String) async -> Result<String, PowerPointParserError> {
        guard fileManager.fileExists(atPath: path) else {
            return .failure(.fileNotFound)
        }

        let tempDir = fileManager.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)

        do {
            try fileManager.createDirectory(at: tempDir, withIntermediateDirectories: true)

            // Extract using native unzip
            let success = NativeUnzipper.unzipFile(atPath: path,
                                                   toDestination: tempDir.path)

            if success {
                return .success(tempDir.path)
            } else {
                return .failure(.extractionFailed(underlying: NSError(domain: "ZipArchive", code: -1)))
            }
        } catch {
            return .failure(.extractionFailed(underlying: error))
        }
    }

    /// Parse presentation and detect slide count
    func parse(presentationAt path: String) async -> Result<PresentationMetadata, PowerPointParserError> {
        // Extract archive
        let extractResult = await extractArchive(at: path)
        guard case .success(let extractedPath) = extractResult else {
            if case .failure(let error) = extractResult {
                return .failure(error)
            }
            return .failure(.extractionFailed(underlying: NSError(domain: "Unknown", code: -1)))
        }

        // Count slides in ppt/slides/ directory
        let slidesDir = URL(fileURLWithPath: extractedPath)
            .appendingPathComponent("ppt/slides")

        do {
            let files = try fileManager.contentsOfDirectory(atPath: slidesDir.path)
            let slideFiles = files.filter { $0.hasPrefix("slide") && $0.hasSuffix(".xml") }

            let metadata = PresentationMetadata(
                slideCount: slideFiles.count,
                filename: URL(fileURLWithPath: path).lastPathComponent,
                filePath: path
            )

            return .success(metadata)
        } catch {
            return .failure(.parsingFailed(underlying: error))
        }
    }

    /// Parse individual slide XML
    func parseSlide(at presentationPath: String, slideNumber: Int) async -> Result<SlideData, PowerPointParserError> {
        // Extract archive
        let extractResult = await extractArchive(at: presentationPath)
        guard case .success(let extractedPath) = extractResult else {
            if case .failure(let error) = extractResult {
                return .failure(error)
            }
            return .failure(.extractionFailed(underlying: NSError(domain: "Unknown", code: -1)))
        }

        // Read slide XML
        let slideFile = URL(fileURLWithPath: extractedPath)
            .appendingPathComponent("ppt/slides/slide\(slideNumber).xml")

        guard let xmlData = try? Data(contentsOf: slideFile) else {
            return .failure(.fileNotFound)
        }

        // Parse XML and extract text
        let xmlString = String(data: xmlData, encoding: .utf8) ?? ""
        let textExtractor = TextExtractor()
        let textResult = textExtractor.extractText(from: xmlString)

        let textContent: String
        switch textResult {
        case .success(let text):
            textContent = text
        case .failure:
            textContent = ""
        }

        // Detect images and media (simplified check for <p:pic> and <p:video> tags)
        let hasImages = xmlString.contains("<p:pic>")
        let hasMedia = xmlString.contains("<p:video>") || xmlString.contains("<p:audio>")

        let slideData = SlideData(
            slideNumber: Int16(slideNumber),
            textContent: textContent,
            thumbnailPath: "", // Will be set by thumbnail generator
            hasImages: hasImages,
            hasMedia: hasMedia,
            slideLayout: nil
        )

        return .success(slideData)
    }
}
