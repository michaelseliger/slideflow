//
//  DirectoryListView.swift
//  Slideflow
//
//  List of configured directories with indexing progress
//

import SwiftUI
import CoreData

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
        NavigationView {
            List {
                ForEach(directories) { directory in
                    DirectoryRow(directory: directory, onDelete: {
                        selectedDirectory = directory
                        showingDeleteConfirmation = true
                    })
                }
            }
            .navigationTitle("Configured Directories")
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button(action: { showingAddDirectory = true }) {
                        Label("Add Directory", systemImage: "folder.badge.plus")
                    }
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
    }

    private func deleteDirectory(_ directory: DirectoryConfig) {
        withAnimation {
            viewContext.delete(directory)
            _ = CoreDataStack.shared.save(context: viewContext)
        }
    }
}

struct DirectoryRow: View {
    @ObservedObject var directory: DirectoryConfig
    let onDelete: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Image(systemName: "folder.fill")
                    .foregroundColor(.blue)

                Text(directory.path)
                    .font(.body)

                Spacer()

                if directory.isActive {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundColor(.green)
                }
            }

            HStack {
                Text("\(directory.slideCount) slides")
                    .font(.caption)
                    .foregroundColor(.secondary)

                Spacer()

                if let lastScan = directory.lastScanDate {
                    Text("Last scan: \(lastScan, style: .relative)")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }

            // Show indexing progress if active
            if directory.isActive {
                IndexingProgressView(directoryId: directory.directoryId)
            }
        }
        .padding(.vertical, 4)
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            Button(role: .destructive, action: onDelete) {
                Label("Delete", systemImage: "trash")
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
            HStack {
                ProgressView()
                    .scaleEffect(0.7)
                Text("Indexing... \(indexingCount)/\(totalCount) presentations")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
        }
    }
}
