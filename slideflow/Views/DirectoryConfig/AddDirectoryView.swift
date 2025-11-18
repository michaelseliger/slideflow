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
        VStack(spacing: 24) {
            // Icon and title
            VStack(spacing: 8) {
                Image(systemName: "folder.badge.plus")
                    .font(.system(size: 48))
                    .foregroundColor(.brandPrimary)

                Text("Add Slide Directory")
                    .font(.title2.bold())

                Text("Choose a folder containing PowerPoint presentations")
                    .font(.subheadline)
                    .foregroundColor(.secondary)
                    .multilineTextAlignment(.center)
            }

            // Selected path display
            if !selectedPath.isEmpty {
                ModernCard {
                    HStack(spacing: 12) {
                        Image(systemName: "folder.fill")
                            .foregroundColor(.brandPrimary)
                            .font(.system(size: 20))

                        VStack(alignment: .leading, spacing: 4) {
                            Text("Selected Location")
                                .font(.caption)
                                .fontWeight(.semibold)
                                .foregroundColor(.secondary)

                            Text(selectedPath)
                                .font(.body)
                                .lineLimit(2)
                                .truncationMode(.middle)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }

            // Choose button
            PrimaryButton("Choose Directory...", icon: "folder") {
                selectDirectory()
            }
            .disabled(isIndexing)

            // Error message
            if let error = errorMessage {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.caption)
                    Text(error)
                        .font(.caption)
                }
                .foregroundColor(.red)
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.red.opacity(0.08))
                .cornerRadius(8)
            }

            Divider()

            // Actions
            HStack(spacing: 12) {
                SecondaryButton("Cancel") {
                    dismiss()
                }
                .keyboardShortcut(.cancelAction)

                Spacer()

                if isIndexing {
                    HStack(spacing: 8) {
                        ProgressView()
                            .scaleEffect(0.8)
                        Text("Indexing...")
                            .font(.subheadline)
                            .foregroundColor(.secondary)
                    }
                } else {
                    PrimaryButton("Add & Index", icon: "arrow.down.doc") {
                        addDirectory()
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(selectedPath.isEmpty)
                }
            }
        }
        .padding(32)
        .frame(width: 540)
        .background(Color.backgroundCard)
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
