//
//  AddDirectoryView.swift
//  Slideflow
//
//  Add directory dialog with NSOpenPanel and TCC handling
//

import SwiftUI
import AppKit

struct AddDirectoryView: View {
    @Environment(\.managedObjectContext) private var viewContext
    @Environment(\.dismiss) private var dismiss

    @State private var selectedPath: String = ""
    @State private var errorMessage: String?
    @State private var isIndexing = false

    var body: some View {
        VStack(spacing: 20) {
            Text("Add Directory")
                .font(.headline)

            if let path = selectedPath.isEmpty ? nil : selectedPath {
                HStack {
                    Image(systemName: "folder")
                    Text(path)
                        .font(.body)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                .padding()
                .background(Color.secondary.opacity(0.1))
                .cornerRadius(8)
            }

            Button("Choose Directory...") {
                selectDirectory()
            }
            .disabled(isIndexing)

            if let error = errorMessage {
                Text(error)
                    .foregroundColor(.red)
                    .font(.caption)
            }

            HStack {
                Button("Cancel") {
                    dismiss()
                }
                .keyboardShortcut(.cancelAction)

                Spacer()

                Button("Add") {
                    addDirectory()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(selectedPath.isEmpty || isIndexing)
            }
            .padding(.top)
        }
        .padding()
        .frame(width: 500, height: 250)
    }

    private func selectDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = false
        panel.prompt = "Select"
        panel.message = "Choose a directory containing PowerPoint files"

        panel.begin { response in
            if response == .OK, let url = panel.url {
                // Start accessing to create bookmark
                _ = url.startAccessingSecurityScopedResource()
                selectedPath = url.path
                errorMessage = nil
            }
        }
    }

    private func addDirectory() {
        guard !selectedPath.isEmpty else { return }

        // Check if directory already exists
        let fetchRequest = NSFetchRequest<DirectoryConfig>(entityName: "DirectoryConfig")
        fetchRequest.predicate = NSPredicate(format: "path == %@", selectedPath)

        do {
            let existing = try viewContext.fetch(fetchRequest)
            if !existing.isEmpty {
                errorMessage = "This directory is already configured"
                return
            }
        } catch {
            errorMessage = "Failed to check existing directories: \(error.localizedDescription)"
            return
        }

        // Create new directory config
        let directory = DirectoryConfig(context: viewContext)
        directory.directoryId = UUID()
        directory.path = selectedPath
        directory.addedDate = Date()
        directory.isActive = true
        directory.slideCount = 0

        // Create security bookmark for persistent access
        let dirURL = URL(fileURLWithPath: selectedPath)
        do {
            let bookmarkData = try dirURL.bookmarkData(
                options: .withSecurityScope,
                includingResourceValuesForKeys: nil,
                relativeTo: nil
            )
            directory.securityBookmark = bookmarkData
            print("✅ Created directory bookmark for \(selectedPath)")
        } catch {
            print("⚠️ Failed to create directory bookmark: \(error)")
            errorMessage = "Warning: Directory access may not persist across app launches"
        }

        let saveResult = CoreDataStack.shared.save(context: viewContext)
        switch saveResult {
        case .success:
            // Start indexing in background
            Task {
                await startIndexing(directory: directory)
            }
            dismiss()

        case .failure(let error):
            errorMessage = "Failed to save: \(error.localizedDescription)"
        }
    }

    private func startIndexing(directory: DirectoryConfig) async {
        isIndexing = true

        let backgroundContext = CoreDataStack.shared.newBackgroundContext()

        // Fetch the directory in the background context
        guard let directoryId = directory.directoryId as UUID?,
              let bgDirectory = try? backgroundContext.existingObject(with: directory.objectID) as? DirectoryConfig else {
            errorMessage = "Failed to access directory"
            isIndexing = false
            return
        }

        let scanner = DirectoryScanner()
        let indexer = SlideIndexer()

        // Scan for PPTX files
        let scanResult = await scanner.scanDirectory(at: bgDirectory.path)

        guard case .success(let files) = scanResult else {
            errorMessage = "Failed to scan directory"
            isIndexing = false
            return
        }

        // Index each presentation and link to directory
        var totalSlides = 0
        for file in files {
            let result = await indexer.indexPresentation(at: file, in: backgroundContext)
            if case .success(let count) = result {
                totalSlides += count

                // Link presentation to directory
                await linkPresentationToDirectory(filePath: file, directory: bgDirectory, context: backgroundContext)
            }
        }

        // Update directory slide count
        await MainActor.run {
            backgroundContext.perform {
                bgDirectory.slideCount = Int32(totalSlides)
                bgDirectory.lastScanDate = Date()
                _ = CoreDataStack.shared.save(context: backgroundContext)
            }
        }

        isIndexing = false
    }

    private func linkPresentationToDirectory(filePath: String, directory: DirectoryConfig, context: NSManagedObjectContext) async {
        await context.perform {
            let fetchRequest = NSFetchRequest<SourcePresentation>(entityName: "SourcePresentation")
            fetchRequest.predicate = NSPredicate(format: "filePath == %@", filePath)

            if let presentation = try? context.fetch(fetchRequest).first {
                presentation.directory = directory
                _ = CoreDataStack.shared.save(context: context)
            }
        }
    }
}
