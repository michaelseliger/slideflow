//
//  TextExtractorTests.swift
//  SlideflowTests
//
//  Tests for extracting text content from slide XML <a:t> tags
//

import XCTest
@testable import Slideflow

final class TextExtractorTests: XCTestCase {
    var extractor: TextExtractor?

    override func setUp() async throws {
        extractor = TextExtractor()
    }

    func testExtractText_SlideWithText_ExtractsAllText() async throws {
        // GIVEN: Slide XML with known text content
        let slideXML = """
        <p:sld xmlns:p="..." xmlns:a="...">
            <p:cSld>
                <p:spTree>
                    <p:sp>
                        <p:txBody>
                            <a:p>
                                <a:r>
                                    <a:t>Hello World</a:t>
                                </a:r>
                            </a:p>
                        </p:txBody>
                    </p:sp>
                </p:spTree>
            </p:cSld>
        </p:sld>
        """

        // WHEN: Extracting text
        let result = extractor?.extractText(from: slideXML)

        // THEN: Should extract "Hello World"
        switch result {
        case .success(let text):
            XCTAssertTrue(text.contains("Hello World"))
        case .failure(let error):
            XCTFail("Failed to extract text: \(error)")
        case .none:
            XCTFail("Extractor is nil")
        }
    }

    func testExtractText_MultipleTextBoxes_CombinesAllText() async throws {
        // GIVEN: Slide with multiple text boxes
        let slideXML = """
        <p:sld xmlns:p="..." xmlns:a="...">
            <p:cSld>
                <p:spTree>
                    <p:sp>
                        <p:txBody><a:p><a:r><a:t>Title Text</a:t></a:r></a:p></p:txBody>
                    </p:sp>
                    <p:sp>
                        <p:txBody><a:p><a:r><a:t>Body Text</a:t></a:r></a:p></p:txBody>
                    </p:sp>
                </p:spTree>
            </p:cSld>
        </p:sld>
        """

        // WHEN: Extracting text
        let result = extractor?.extractText(from: slideXML)

        // THEN: Should combine all text
        switch result {
        case .success(let text):
            XCTAssertTrue(text.contains("Title Text"))
            XCTAssertTrue(text.contains("Body Text"))
        case .failure(let error):
            XCTFail("Failed to extract text: \(error)")
        case .none:
            XCTFail("Extractor is nil")
        }
    }

    func testExtractText_ImageOnlySlide_ReturnsEmptyString() async throws {
        // GIVEN: Slide with no text (image only)
        let slideXML = """
        <p:sld xmlns:p="..." xmlns:a="...">
            <p:cSld>
                <p:spTree>
                    <p:pic>
                        <!-- Image content -->
                    </p:pic>
                </p:spTree>
            </p:cSld>
        </p:sld>
        """

        // WHEN: Extracting text
        let result = extractor?.extractText(from: slideXML)

        // THEN: Should return empty string
        switch result {
        case .success(let text):
            XCTAssertTrue(text.isEmpty, "Image-only slide should have no text")
        case .failure(let error):
            XCTFail("Failed to extract text: \(error)")
        case .none:
            XCTFail("Extractor is nil")
        }
    }
}
