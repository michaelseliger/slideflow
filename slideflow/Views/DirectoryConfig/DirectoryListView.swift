//
//  DirectoryListView.swift
//  Slideflow
//
//  List of configured directories with indexing progress
//

import SwiftUI
import CoreData

// Import indexing services (defined in SlideIndexer.swift)
// DirectoryScanner and SlideIndexer are accessible from this file

struct DirectoryListView: View {
    @Environment(\.managedObjectContext) private var viewContext
    @FetchRequest(
        sortDescriptors: [NSSortDescriptor(keyPath: \DirectoryConfig.addedDate, ascending: false)],
        animation: .default)
    private var directories: FetchedResults<DirectoryConfig>

    @State private var showingAddDirectory = false
    @State private var selectedDirectory: DirectoryConfig?
    @State private var showingDeleteConfirmation = false

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text("My Slide Directories")
                        .font(.system(size: 24, weight: .bold))

                    Text("\(directories.count) \(directories.count == 1 ? "directory" : "directories") configured")
                        .font(.system(size: 14))
                        .foregroundColor(.secondary)
                }

                Spacer()

                PrimaryButton("Add Directory", icon: "folder.badge.plus") {
                    showingAddDirectory = true
                }
            }
            .padding(24)
            .background(Color.backgroundCard)
            .overlay(
                Rectangle()
                    .fill(Color.borderLight)
                    .frame(height: 1),
                alignment: .bottom
            )

            // Content
            if directories.isEmpty {
                EmptyStateView(
                    icon: "folder.badge.plus",
                    title: "No Directories Configured",
                    message: "Add a directory containing PowerPoint files to get started",
                    actionTitle: "Add Directory",
                    action: { showingAddDirectory = true }
                )
            } else {
                ScrollView {
                    VStack(spacing: 16) {
                        ForEach(directories) { directory in
                            DirectoryCard(directory: directory, onDelete: {
                                selectedDirectory = directory
                                showingDeleteConfirmation = true
                            })
                        }
                    }
                    .padding(24)
                }
                .background(Color.backgroundPrimary)
            }
        }
        .sheet(isPresented: $showingAddDirectory) {
            AddDirectoryView()
        }
        .alert("Remove Directory", isPresented: $showingDeleteConfirmation, presenting: selectedDirectory) { directory in
            Button("Cancel", role: .cancel) {}
            Button("Remove", role: .destructive) {
                deleteDirectory(directory)
            }
        } message: { directory in
            Text("Are you sure you want to remove \(directory.path)? This will also remove all indexed slides from this directory.")
        }
    }

    private func deleteDirectory(_ directory: DirectoryConfig) {
        withAnimation {
            viewContext.delete(directory)
            _ = CoreDataStack.shared.save(context: viewContext)
        }
    }
}

struct DirectoryCard: View {
    @ObservedObject var directory: DirectoryConfig
    let onDelete: () -> Void
    @State private var isRefreshing = false

    var body: some View {
        ModernCard {
            VStack(alignment: .leading, spacing: 16) {
                // Header with path and status
                HStack(spacing: 12) {
                    Image(systemName: "folder.fill")
                        .font(.system(size: 32))
                        .foregroundColor(.brandPrimary)

                    VStack(alignment: .leading, spacing: 4) {
                        Text(URL(fileURLWithPath: directory.path).lastPathComponent)
                            .font(.system(size: 16, weight: .semibold))
                            .foregroundColor(.primary)

                        Text(directory.path)
                            .font(.caption)
                            .foregroundColor(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }

                    Spacer()

                    // Refresh button
                    Button(action: {
                        Task {
                            await refreshIndex()
                        }
                    }) {
                        if isRefreshing {
                            ProgressView()
                                .scaleEffect(0.7)
                                .frame(width: 16, height: 16)
                        } else {
                            Image(systemName: "arrow.clockwise")
                                .foregroundColor(.brandPrimary)
                                .font(.system(size: 16))
                        }
                    }
                    .buttonStyle(.plain)
                    .help("Refresh index")
                    .disabled(isRefreshing)

                    // Delete button
                    Button(action: onDelete) {
                        Image(systemName: "trash")
                            .foregroundColor(.red.opacity(0.8))
                            .font(.system(size: 16))
                    }
                    .buttonStyle(.plain)
                    .help("Remove directory")
                }

                Divider()

                // Stats and status
                HStack(spacing: 24) {
                    // Slide count
                    HStack(spacing: 8) {
                        Image(systemName: "doc.on.doc.fill")
                            .foregroundColor(.brandPrimary.opacity(0.7))
                        VStack(alignment: .leading, spacing: 2) {
                            Text("\(directory.slideCount)")
                                .font(.system(size: 18, weight: .bold))
                            Text("slides")
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                    }

                    Divider()
                        .frame(height: 30)

                    // Last scan
                    HStack(spacing: 8) {
                        Image(systemName: "clock.fill")
                            .foregroundColor(.brandPrimary.opacity(0.7))
                        VStack(alignment: .leading, spacing: 2) {
                            if let lastScan = directory.lastScanDate {
                                Text(lastScan, style: .relative)
                                    .font(.system(size: 14, weight: .medium))
                            } else {
                                Text("Never")
                                    .font(.system(size: 14, weight: .medium))
                            }
                            Text("last scan")
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                    }

                    Spacer()

                    // Status badge
                    if directory.isActive {
                        Badge(text: "Active", color: .brandSuccess)
                    }
                }

                // Show indexing progress if active
                if directory.isActive {
                    IndexingProgressView(directoryId: directory.directoryId)
                }
            }
        }
    }

    private func refreshIndex() async {
        isRefreshing = true

        let backgroundContext = CoreDataStack.shared.newBackgroundContext()

        // Fetch the directory in the background context
        guard let bgDirectory = try? backgroundContext.existingObject(with: directory.objectID) as? DirectoryConfig else {
            isRefreshing = false
            return
        }

        let scanner = DirectoryScanner()
        let indexer = SlideIndexer()

        // Scan for PPTX files
        let scanResult = await scanner.scanDirectory(at: bgDirectory.path)

        guard case .success(let files) = scanResult else {
            isRefreshing = false
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

        // Update directory slide count and last scan date
        await MainActor.run {
            backgroundContext.perform {
                bgDirectory.slideCount = Int32(totalSlides)
                bgDirectory.lastScanDate = Date()
                _ = CoreDataStack.shared.save(context: backgroundContext)
            }
        }

        isRefreshing = false
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

struct IndexingProgressView: View {
    let directoryId: UUID

    @FetchRequest private var presentations: FetchedResults<SourcePresentation>

    init(directoryId: UUID) {
        self.directoryId = directoryId
        _presentations = FetchRequest(
            sortDescriptors: [],
            predicate: NSPredicate(format: "directory.directoryId == %@", directoryId as CVarArg)
        )
    }

    var indexingCount: Int {
        presentations.filter { $0.indexingStatus == "indexing" }.count
    }

    var totalCount: Int {
        presentations.count
    }

    var body: some View {
        if indexingCount > 0 {
            HStack(spacing: 8) {
                ProgressView()
                    .scaleEffect(0.7)
                    .tint(.brandPrimary)
                Text("Indexing \(indexingCount) of \(totalCount) presentations...")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.backgroundSubtle)
            .cornerRadius(8)
        }
    }
}
