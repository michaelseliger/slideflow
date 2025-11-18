//
//  PPTXMerger.swift
//  Slideflow
//
//  Native Swift PPTX slide merger - no Python dependencies
//

import Foundation

enum PPTXMergerError: Error {
    case invalidPPTX(path: String)
    case extractionFailed
    case mergeOperationFailed(reason: String)
    case zipFailed
}

/// Native Swift PPTX merger
/// Extracts specific slides from multiple PPTX files and merges them
class PPTXMerger {
    private let fileManager = FileManager.default

    /// Merge specific slides into new PPTX
    /// - Parameters:
    ///   - specs: Array of (sourcePath, slideNumbers) tuples
    ///   - outputPath: Destination file path
    func mergeSlides(
        specs: [(sourcePath: String, slideNumbers: [Int])],
        to outputPath: String
    ) async -> Result<Void, PPTXMergerError> {

        let tempDir = fileManager.temporaryDirectory
            .appendingPathComponent("slideflow_merge_\(UUID().uuidString)")

        do {
            try fileManager.createDirectory(at: tempDir, withIntermediateDirectories: true)

            // Use first source presentation as template
            guard let firstSpec = specs.first else {
                return .failure(.mergeOperationFailed(reason: "No slides to merge"))
            }

            // Create base PPTX structure using first presentation as template
            let baseStructure = try await createBasePPTXStructure(at: tempDir, templatePath: firstSpec.sourcePath)

            var slideCounter = 1
            var allMediaFiles: [String: URL] = [:] // Original path -> temp path

            // Extract and merge each slide
            for (sourcePath, slideNumbers) in specs {
                let extractResult = await extractSlides(
                    from: sourcePath,
                    slideNumbers: slideNumbers,
                    into: baseStructure,
                    startingIndex: slideCounter,
                    mediaFiles: &allMediaFiles
                )

                guard case .success(let count) = extractResult else {
                    if case .failure(let error) = extractResult {
                        return .failure(error)
                    }
                    return .failure(.mergeOperationFailed(reason: "Failed to extract slides"))
                }

                slideCounter += count
            }

            // Update presentation.xml with slide list
            try updatePresentationXML(
                at: baseStructure,
                slideCount: slideCounter - 1
            )

            // Update [Content_Types].xml with all slides and layouts
            try updateContentTypes(at: baseStructure, slideCount: slideCounter - 1)

            // Zip into final PPTX
            let zipResult = zipDirectory(tempDir, to: outputPath)
            guard case .success = zipResult else {
                if case .failure(let error) = zipResult {
                    return .failure(error)
                }
                return .failure(.zipFailed)
            }

            // Cleanup temp directory
            try? fileManager.removeItem(at: tempDir)

            return .success(())

        } catch {
            return .failure(.mergeOperationFailed(reason: error.localizedDescription))
        }
    }

    /// Create base PPTX directory structure
    private func createBasePPTXStructure(at baseDir: URL, templatePath: String) async throws -> URL {
        let pptDir = baseDir.appendingPathComponent("ppt")
        let slidesDir = pptDir.appendingPathComponent("slides")
        let slidesRelsDir = slidesDir.appendingPathComponent("_rels")
        let mediaDir = pptDir.appendingPathComponent("media")
        let relsDir = pptDir.appendingPathComponent("_rels")
        let mastersDir = pptDir.appendingPathComponent("slideMasters")
        let mastersRelsDir = mastersDir.appendingPathComponent("_rels")
        let layoutsDir = pptDir.appendingPathComponent("slideLayouts")
        let layoutsRelsDir = layoutsDir.appendingPathComponent("_rels")
        let themeDir = pptDir.appendingPathComponent("theme")
        let docPropsDir = baseDir.appendingPathComponent("docProps")

        // Create directories
        for dir in [pptDir, slidesDir, slidesRelsDir, mediaDir, relsDir, mastersDir, mastersRelsDir, layoutsDir, layoutsRelsDir, themeDir, docPropsDir] {
            try fileManager.createDirectory(at: dir, withIntermediateDirectories: true)
        }

        // Copy slideMasters, slideLayouts, and theme from first presentation template
        let templateTempDir = fileManager.temporaryDirectory
            .appendingPathComponent("template_\(UUID().uuidString)")

        let unzipSuccess = NativeUnzipper.unzipFile(
            atPath: templatePath,
            toDestination: templateTempDir.path
        )

        guard unzipSuccess else {
            throw PPTXMergerError.extractionFailed
        }

        // Copy masters
        let srcMasters = templateTempDir.appendingPathComponent("ppt/slideMasters")
        if fileManager.fileExists(atPath: srcMasters.path) {
            try? fileManager.contentsOfDirectory(atPath: srcMasters.path).forEach { file in
                let src = srcMasters.appendingPathComponent(file)
                let dst = mastersDir.appendingPathComponent(file)
                try? fileManager.copyItem(at: src, to: dst)
            }

            // Copy master relationships
            let srcMastersRels = srcMasters.appendingPathComponent("_rels")
            if fileManager.fileExists(atPath: srcMastersRels.path) {
                try? fileManager.contentsOfDirectory(atPath: srcMastersRels.path).forEach { file in
                    let src = srcMastersRels.appendingPathComponent(file)
                    let dst = mastersRelsDir.appendingPathComponent(file)
                    try? fileManager.copyItem(at: src, to: dst)
                }
            }
        }

        // Copy layouts
        let srcLayouts = templateTempDir.appendingPathComponent("ppt/slideLayouts")
        if fileManager.fileExists(atPath: srcLayouts.path) {
            try? fileManager.contentsOfDirectory(atPath: srcLayouts.path).forEach { file in
                let src = srcLayouts.appendingPathComponent(file)
                let dst = layoutsDir.appendingPathComponent(file)
                try? fileManager.copyItem(at: src, to: dst)
            }

            // Copy layout relationships
            let srcLayoutsRels = srcLayouts.appendingPathComponent("_rels")
            if fileManager.fileExists(atPath: srcLayoutsRels.path) {
                try? fileManager.contentsOfDirectory(atPath: srcLayoutsRels.path).forEach { file in
                    let src = srcLayoutsRels.appendingPathComponent(file)
                    let dst = layoutsRelsDir.appendingPathComponent(file)
                    try? fileManager.copyItem(at: src, to: dst)
                }
            }
        }

        // Copy theme
        let srcTheme = templateTempDir.appendingPathComponent("ppt/theme")
        if fileManager.fileExists(atPath: srcTheme.path) {
            try? fileManager.contentsOfDirectory(atPath: srcTheme.path).forEach { file in
                let src = srcTheme.appendingPathComponent(file)
                let dst = themeDir.appendingPathComponent(file)
                try? fileManager.copyItem(at: src, to: dst)
            }
        }

        // Copy docProps
        let srcDocProps = templateTempDir.appendingPathComponent("docProps")
        if fileManager.fileExists(atPath: srcDocProps.path) {
            try? fileManager.contentsOfDirectory(atPath: srcDocProps.path).forEach { file in
                let src = srcDocProps.appendingPathComponent(file)
                let dst = docPropsDir.appendingPathComponent(file)
                try? fileManager.copyItem(at: src, to: dst)
            }
        } else {
            // Create minimal docProps if not found
            let appXML = """
            <?xml version="1.0" encoding="UTF-8" standalone="yes"?>
            <Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
                <Application>Slideflow</Application>
                <PresentationFormat>On-screen Show (4:3)</PresentationFormat>
                <Slides>1</Slides>
            </Properties>
            """

            let coreXML = """
            <?xml version="1.0" encoding="UTF-8" standalone="yes"?>
            <cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
                <dc:title>Merged Presentation</dc:title>
                <dc:creator>Slideflow</dc:creator>
                <cp:lastModifiedBy>Slideflow</cp:lastModifiedBy>
                <dcterms:created xsi:type="dcterms:W3CDTF">\(ISO8601DateFormatter().string(from: Date()))</dcterms:created>
                <dcterms:modified xsi:type="dcterms:W3CDTF">\(ISO8601DateFormatter().string(from: Date()))</dcterms:modified>
            </cp:coreProperties>
            """

            try? appXML.write(to: docPropsDir.appendingPathComponent("app.xml"), atomically: true, encoding: .utf8)
            try? coreXML.write(to: docPropsDir.appendingPathComponent("core.xml"), atomically: true, encoding: .utf8)
        }

        // Copy presentation.xml from template (preserve complete structure)
        let srcPresentation = templateTempDir.appendingPathComponent("ppt/presentation.xml")
        let dstPresentation = pptDir.appendingPathComponent("presentation.xml")
        if fileManager.fileExists(atPath: srcPresentation.path) {
            try? fileManager.copyItem(at: srcPresentation, to: dstPresentation)
        }

        // Copy presentation.xml.rels from template
        let srcPresentationRels = templateTempDir.appendingPathComponent("ppt/_rels/presentation.xml.rels")
        let dstPresentationRels = relsDir.appendingPathComponent("presentation.xml.rels")
        if fileManager.fileExists(atPath: srcPresentationRels.path) {
            try? fileManager.copyItem(at: srcPresentationRels, to: dstPresentationRels)
        }

        // Copy support files (viewProps, presProps, tableStyles)
        for file in ["viewProps.xml", "presProps.xml", "tableStyles.xml"] {
            let src = templateTempDir.appendingPathComponent("ppt/\(file)")
            let dst = pptDir.appendingPathComponent(file)
            if fileManager.fileExists(atPath: src.path) {
                try? fileManager.copyItem(at: src, to: dst)
            }
        }

        // Copy notesMasters if present
        let srcNotesMasters = templateTempDir.appendingPathComponent("ppt/notesMasters")
        if fileManager.fileExists(atPath: srcNotesMasters.path) {
            let dstNotesMasters = pptDir.appendingPathComponent("notesMasters")
            try? fileManager.copyItem(at: srcNotesMasters, to: dstNotesMasters)
        }

        // Copy [Content_Types].xml from template (preserve complete structure)
        let srcContentTypes = templateTempDir.appendingPathComponent("[Content_Types].xml")
        let dstContentTypes = baseDir.appendingPathComponent("[Content_Types].xml")
        if fileManager.fileExists(atPath: srcContentTypes.path) {
            try? fileManager.copyItem(at: srcContentTypes, to: dstContentTypes)
        }

        // Copy _rels/.rels from template
        let baseRelsDir = baseDir.appendingPathComponent("_rels")
        try? fileManager.createDirectory(at: baseRelsDir, withIntermediateDirectories: true)

        let srcBaseRels = templateTempDir.appendingPathComponent("_rels/.rels")
        let dstBaseRels = baseRelsDir.appendingPathComponent(".rels")
        if fileManager.fileExists(atPath: srcBaseRels.path) {
            try? fileManager.copyItem(at: srcBaseRels, to: dstBaseRels)
        }

        // Cleanup template temp
        try? fileManager.removeItem(at: templateTempDir)

        return baseDir
    }

    /// Extract specific slides from source PPTX
    private func extractSlides(
        from sourcePath: String,
        slideNumbers: [Int],
        into baseStructure: URL,
        startingIndex: Int,
        mediaFiles: inout [String: URL]
    ) async -> Result<Int, PPTXMergerError> {

        // Unzip source PPTX
        let sourceTempDir = fileManager.temporaryDirectory
            .appendingPathComponent("source_\(UUID().uuidString)")

        let unzipResult = NativeUnzipper.unzipFile(
            atPath: sourcePath,
            toDestination: sourceTempDir.path
        )

        guard unzipResult else {
            return .failure(.extractionFailed)
        }

        var extractedCount = 0

        // Copy each requested slide
        for (offset, slideNum) in slideNumbers.enumerated() {
            let sourceSlideFile = sourceTempDir
                .appendingPathComponent("ppt/slides/slide\(slideNum).xml")

            guard fileManager.fileExists(atPath: sourceSlideFile.path) else {
                print("⚠️ Slide \(slideNum) not found in \(sourcePath)")
                continue
            }

            let destSlideNum = startingIndex + offset
            let destSlideFile = baseStructure
                .appendingPathComponent("ppt/slides/slide\(destSlideNum).xml")

            // Copy slide XML
            try? fileManager.copyItem(at: sourceSlideFile, to: destSlideFile)

            // Copy slide relationships if they exist
            let sourceRels = sourceTempDir
                .appendingPathComponent("ppt/slides/_rels/slide\(slideNum).xml.rels")

            if fileManager.fileExists(atPath: sourceRels.path) {
                let destRels = baseStructure
                    .appendingPathComponent("ppt/slides/_rels/slide\(destSlideNum).xml.rels")
                try? fileManager.copyItem(at: sourceRels, to: destRels)
            }

            extractedCount += 1
        }

        // Copy media directory if it exists
        let sourceMedia = sourceTempDir.appendingPathComponent("ppt/media")
        if fileManager.fileExists(atPath: sourceMedia.path) {
            let destMedia = baseStructure.appendingPathComponent("ppt/media")

            if let mediaContents = try? fileManager.contentsOfDirectory(at: sourceMedia, includingPropertiesForKeys: nil) {
                for mediaFile in mediaContents {
                    let destFile = destMedia.appendingPathComponent(mediaFile.lastPathComponent)
                    if !fileManager.fileExists(atPath: destFile.path) {
                        try? fileManager.copyItem(at: mediaFile, to: destFile)
                    }
                }
            }
        }

        // Cleanup source temp
        try? fileManager.removeItem(at: sourceTempDir)

        return .success(extractedCount)
    }

    /// Update presentation.xml with slide references
    private func updatePresentationXML(at baseStructure: URL, slideCount: Int) throws {
        let presentationFile = baseStructure.appendingPathComponent("ppt/presentation.xml")

        // Parse existing presentation.xml
        let doc = try parseXML(at: presentationFile)
        guard let root = doc.rootElement() else {
            throw PPTXMergerError.mergeOperationFailed(reason: "Invalid presentation.xml")
        }

        // Define namespaces
        let pNS = "http://schemas.openxmlformats.org/presentationml/2006/main"
        let rNS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"

        // Find and update sldIdLst
        if let sldIdLst = root.elements(forLocalName: "sldIdLst", uri: pNS).first {
            // Clear existing slides
            sldIdLst.setChildren([])

            // Add new slides (slides start at rId2, rId1 is reserved for slideMaster)
            for i in 1...slideCount {
                let slideId = 256 + i
                let rId = i + 1 // rId2, rId3, etc.

                let sldId = XMLElement(name: "p:sldId")
                sldId.addAttribute(XMLNode.attribute(withName: "id", stringValue: "\(slideId)") as! XMLNode)
                sldId.addAttribute(XMLNode.attribute(withName: "r:id", stringValue: "rId\(rId)") as! XMLNode)
                sldIdLst.addChild(sldId)
            }
        }

        // Save updated presentation.xml
        try saveXML(doc, to: presentationFile)

        // Update presentation.xml.rels
        try updatePresentationRels(at: baseStructure, slideCount: slideCount)
    }

    /// Update presentation.xml.rels with slide relationships
    private func updatePresentationRels(at baseStructure: URL, slideCount: Int) throws {
        let relsFile = baseStructure.appendingPathComponent("ppt/_rels/presentation.xml.rels")

        // Parse existing presentation.xml.rels
        let doc = try parseXML(at: relsFile)
        guard let root = doc.rootElement() else {
            throw PPTXMergerError.mergeOperationFailed(reason: "Invalid presentation.xml.rels")
        }

        let relNS = "http://schemas.openxmlformats.org/package/2006/relationships"

        // Remove existing slide relationships (preserve master, theme, viewProps, etc.)
        let existingRels = root.elements(forLocalName: "Relationship", uri: relNS)
        for rel in existingRels {
            if let type = rel.attribute(forName: "Type")?.stringValue,
               type.contains("/slide") && !type.contains("slideMaster") {
                rel.detach()
            }
        }

        // Add new slide relationships (starting from rId2)
        // Find the first available rId after existing relationships
        var maxRId = 1
        for rel in root.elements(forLocalName: "Relationship", uri: relNS) {
            if let idStr = rel.attribute(forName: "Id")?.stringValue,
               let idNum = Int(idStr.replacingOccurrences(of: "rId", with: "")) {
                maxRId = max(maxRId, idNum)
            }
        }

        // Insert slide relationships after rId1 (slideMaster)
        for i in 1...slideCount {
            let rId = i + 1 // rId2, rId3, etc.
            let rel = XMLElement(name: "Relationship")
            rel.addAttribute(XMLNode.attribute(withName: "Id", stringValue: "rId\(rId)") as! XMLNode)
            rel.addAttribute(XMLNode.attribute(withName: "Type", stringValue: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide") as! XMLNode)
            rel.addAttribute(XMLNode.attribute(withName: "Target", stringValue: "slides/slide\(i).xml") as! XMLNode)

            // Insert after first relationship (slideMaster)
            if i == 1, root.childCount > 1 {
                root.insertChild(rel, at: 1)
            } else {
                root.insertChild(rel, at: i)
            }
        }

        // Save updated presentation.xml.rels
        try saveXML(doc, to: relsFile)
    }

    /// Update [Content_Types].xml with all slides and layouts
    private func updateContentTypes(at baseStructure: URL, slideCount: Int) throws {
        let contentTypesFile = baseStructure.appendingPathComponent("[Content_Types].xml")

        // Parse existing [Content_Types].xml
        let doc = try parseXML(at: contentTypesFile)
        guard let root = doc.rootElement() else {
            throw PPTXMergerError.mergeOperationFailed(reason: "Invalid [Content_Types].xml")
        }

        let ctNS = "http://schemas.openxmlformats.org/package/2006/content-types"

        // Remove existing slide Override elements (preserve all others)
        let existingOverrides = root.elements(forLocalName: "Override", uri: ctNS)
        for override in existingOverrides {
            if let partName = override.attribute(forName: "PartName")?.stringValue,
               partName.contains("/slides/slide") {
                override.detach()
            }
        }

        // Add new slide Override elements
        for i in 1...slideCount {
            let override = XMLElement(name: "Override")
            override.addAttribute(XMLNode.attribute(withName: "PartName", stringValue: "/ppt/slides/slide\(i).xml") as! XMLNode)
            override.addAttribute(XMLNode.attribute(withName: "ContentType", stringValue: "application/vnd.openxmlformats-officedocument.presentationml.slide+xml") as! XMLNode)
            root.addChild(override)
        }

        // Save updated [Content_Types].xml
        try saveXML(doc, to: contentTypesFile)
    }

    /// Zip directory into PPTX file
    private func zipDirectory(_ directory: URL, to outputPath: String) -> Result<Void, PPTXMergerError> {
        // Create zip in temp directory first to avoid permissions issues
        let tempZip = fileManager.temporaryDirectory
            .appendingPathComponent(UUID().uuidString + ".zip")

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/zip")
        process.arguments = [
            "-r",
            "-q",
            tempZip.path,
            "."
        ]
        process.currentDirectoryURL = directory

        let errorPipe = Pipe()
        process.standardError = errorPipe

        do {
            try process.run()
            process.waitUntilExit()

            guard process.terminationStatus == 0 else {
                let errorData = errorPipe.fileHandleForReading.readDataToEndOfFile()
                if let errorOutput = String(data: errorData, encoding: .utf8) {
                    print("❌ Zip error: \(errorOutput)")
                }
                return .failure(.zipFailed)
            }

            // Move from temp to final destination
            let destURL = URL(fileURLWithPath: outputPath)
            try? fileManager.removeItem(at: destURL) // Remove if exists
            try fileManager.moveItem(at: tempZip, to: destURL)

            return .success(())
        } catch {
            print("❌ Zip/move failed: \(error)")
            return .failure(.zipFailed)
        }
    }

    // MARK: - XML Parsing Helpers

    /// Parse XML file into XMLDocument
    private func parseXML(at path: URL) throws -> XMLDocument {
        let data = try Data(contentsOf: path)
        return try XMLDocument(data: data, options: [])
    }

    /// Save XMLDocument to file
    private func saveXML(_ doc: XMLDocument, to path: URL) throws {
        let data = doc.xmlData(options: [.nodePrettyPrint])
        try data.write(to: path)
    }
}
