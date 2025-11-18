#!/usr/bin/swift

// Minimal test to verify PPTX creation issue
import Foundation

let fm = FileManager.default
let testDir = fm.temporaryDirectory.appendingPathComponent("pptx_test")

// Clean and create test directory
try? fm.removeItem(at: testDir)
try! fm.createDirectory(at: testDir, withIntermediateDirectories: true)

// Extract a working PPTX as reference
let sourcePPTX = "/Users/michaelseliger/Downloads/Dickinson_Sample_Slides.pptx"
let unzipCmd = Process()
unzipCmd.currentDirectoryURL = testDir
unzipCmd.executableURL = URL(fileURLWithPath: "/usr/bin/unzip")
unzipCmd.arguments = ["-q", sourcePPTX]
try! unzipCmd.run()
unzipCmd.waitUntilExit()

print("✅ Extracted source PPTX")

// List what we have
let contents = try! fm.contentsOfDirectory(atPath: testDir.path)
print("📁 Top-level items: \(contents)")

// Check critical files
let critical = [
    "[Content_Types].xml",
    "_rels/.rels",
    "ppt/presentation.xml",
    "ppt/_rels/presentation.xml.rels",
    "ppt/slides/slide1.xml"
]

for file in critical {
    let path = testDir.appendingPathComponent(file).path
    if fm.fileExists(atPath: path) {
        print("✅ \(file)")
    } else {
        print("❌ Missing: \(file)")
    }
}

// Re-zip it
let outputPath = "/Users/michaelseliger/Downloads/test_rezip.pptx"
try? fm.removeItem(atPath: outputPath)

let zipCmd = Process()
zipCmd.currentDirectoryURL = testDir
zipCmd.executableURL = URL(fileURLWithPath: "/usr/bin/zip")
zipCmd.arguments = ["-r", "-q", outputPath, "."]
try! zipCmd.run()
zipCmd.waitUntilExit()

print("\n📦 Created: \(outputPath)")
print("Try opening this file to see if it works")