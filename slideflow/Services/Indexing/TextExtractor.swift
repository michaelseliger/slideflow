//
//  TextExtractor.swift
//  Slideflow
//
//  Extracts searchable text from PowerPoint slide XML <a:t> tags
//

import Foundation

enum TextExtractorError: Error {
    case invalidXML
    case parsingFailed(underlying: Error)
}

class TextExtractor {
    init() {}

    /// Extract all text from slide XML
    func extractText(from xmlString: String) -> Result<String, TextExtractorError> {
        guard !xmlString.isEmpty else {
            return .success("")
        }

        var extractedText: [String] = []

        // Simple regex approach to extract <a:t>...</a:t> content
        // In production, would use XMLCoder with proper schema
        let pattern = "<a:t>(.*?)</a:t>"

        do {
            let regex = try NSRegularExpression(pattern: pattern, options: [.dotMatchesLineSeparators])
            let nsString = xmlString as NSString
            let matches = regex.matches(in: xmlString, range: NSRange(location: 0, length: nsString.length))

            for match in matches {
                if match.numberOfRanges > 1 {
                    let textRange = match.range(at: 1)
                    let text = nsString.substring(with: textRange)
                    extractedText.append(text)
                }
            }

            // Join all text with spaces
            let combinedText = extractedText.joined(separator: " ")
            return .success(combinedText)

        } catch {
            return .failure(.parsingFailed(underlying: error))
        }
    }

    /// Extract text with metadata (shape types, etc.)
    func extractTextWithMetadata(from xmlString: String) -> Result<[(text: String, shapeType: String)], TextExtractorError> {
        // Placeholder for more advanced extraction
        let textResult = extractText(from: xmlString)

        switch textResult {
        case .success(let text):
            return .success([(text: text, shapeType: "unknown")])
        case .failure(let error):
            return .failure(error)
        }
    }
}
