//
//  WorkspaceEditorView.swift
//  Slideflow
//
//  Edit workspace slides with drag-and-drop reordering
//

import SwiftUI
import CoreData
import AppKit
import UniformTypeIdentifiers

struct WorkspaceEditorView: View {
    @ObservedObject var workspace: Workspace
    @Environment(\.managedObjectContext) private var viewContext

    @State private var workspaceSlides: [WorkspaceSlide] = []
    @State private var showingExportView = false

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                VStack(alignment: .leading) {
                    Text(workspace.name)
                        .font(.title)

                    Text("\(workspace.slideCount) slides")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }

                Spacer()

                Button("Export") {
                    showingExportView = true
                }
                .keyboardShortcut("e", modifiers: .command)
                .disabled(workspace.slideCount == 0)
            }
            .padding()
            .background(Color.backgroundCard)
            .overlay(
                Rectangle()
                    .fill(Color.borderLight)
                    .frame(height: 1),
                alignment: .bottom
            )

            // Slides
            if workspaceSlides.isEmpty {
                EmptyWorkspaceView()
                    .onDrop(of: [.text, .plainText], isTargeted: nil) { providers in
                        handleDrop(providers: providers)
                        return true
                    }
            } else {
                SlideReorderView(
                    workspaceSlides: $workspaceSlides,
                    onReorder: { from, to in
                        reorderSlides(from: from, to: to)
                    },
                    onDelete: { indexSet in
                        deleteSlides(at: indexSet)
                    }
                )
                .onDrop(of: [.text, .plainText], isTargeted: nil) { providers in
                    handleDrop(providers: providers)
                    return true
                }
            }
        }
        .onAppear {
            loadWorkspaceSlides()
        }
        .sheet(isPresented: $showingExportView) {
            ExportDialogView(workspace: workspace)
        }
    }

    private func loadWorkspaceSlides() {
        let fetchRequest: NSFetchRequest<WorkspaceSlide> = WorkspaceSlide.fetchRequest()
        fetchRequest.predicate = NSPredicate(format: "workspace == %@", workspace)
        fetchRequest.sortDescriptors = [NSSortDescriptor(key: "orderIndex", ascending: true)]

        do {
            workspaceSlides = try viewContext.fetch(fetchRequest)
        } catch {
            print("Failed to load workspace slides: \(error)")
        }
    }

    private func reorderSlides(from source: IndexSet, to destination: Int) {
        var updatedSlides = workspaceSlides
        updatedSlides.move(fromOffsets: source, toOffset: destination)

        // Update order indices
        for (index, slide) in updatedSlides.enumerated() {
            slide.orderIndex = Int16(index)
        }

        workspace.modifiedDate = Date()
        _ = CoreDataStack.shared.save(context: viewContext)

        workspaceSlides = updatedSlides
    }

    private func deleteSlides(at indexSet: IndexSet) {
        // Delete slides from CoreData
        for index in indexSet {
            let slide = workspaceSlides[index]
            viewContext.delete(slide)
        }

        // Update workspace metadata
        workspace.slideCount -= Int32(indexSet.count)
        workspace.modifiedDate = Date()

        // Save deletion
        _ = CoreDataStack.shared.save(context: viewContext)

        // Refresh workspaceSlides from CoreData to get updated list
        let fetchRequest: NSFetchRequest<WorkspaceSlide> = WorkspaceSlide.fetchRequest()
        fetchRequest.predicate = NSPredicate(format: "workspace == %@", workspace)
        fetchRequest.sortDescriptors = [NSSortDescriptor(key: "orderIndex", ascending: true)]

        if let updatedSlides = try? viewContext.fetch(fetchRequest) {
            // Update order indices on remaining slides
            for (index, slide) in updatedSlides.enumerated() {
                slide.orderIndex = Int16(index)
            }

            _ = CoreDataStack.shared.save(context: viewContext)

            // Update local array
            workspaceSlides = updatedSlides
        }
    }

    private func handleDrop(providers: [NSItemProvider]) {
        for provider in providers {
            _ = provider.loadObject(ofClass: NSString.self) { object, error in
                guard let uuidString = object as? String,
                      let slideUUID = UUID(uuidString: uuidString) else {
                    print("Failed to parse UUID from drop: \(error?.localizedDescription ?? "unknown")")
                    return
                }

                DispatchQueue.main.async {
                    self.addSlideToWorkspace(slideId: slideUUID)
                }
            }
        }
    }

    private func addSlideToWorkspace(slideId: UUID) {
        // Fetch the IndexedSlide
        let fetchRequest: NSFetchRequest<IndexedSlide> = IndexedSlide.fetchRequest()
        fetchRequest.predicate = NSPredicate(format: "slideId == %@", slideId as CVarArg)

        guard let indexedSlide = try? viewContext.fetch(fetchRequest).first else {
            print("Failed to find slide with ID: \(slideId)")
            return
        }

        // Check if slide already exists in workspace
        let existingFetch: NSFetchRequest<WorkspaceSlide> = WorkspaceSlide.fetchRequest()
        existingFetch.predicate = NSPredicate(
            format: "workspace == %@ AND slide == %@",
            workspace,
            indexedSlide
        )

        if let existing = try? viewContext.fetch(existingFetch), !existing.isEmpty {
            print("Slide already in workspace")
            return
        }

        // Create WorkspaceSlide
        let workspaceSlide = WorkspaceSlide(context: viewContext)
        workspaceSlide.workspaceSlideId = UUID()
        workspaceSlide.workspace = workspace
        workspaceSlide.slide = indexedSlide
        workspaceSlide.orderIndex = Int16(workspaceSlides.count)
        workspaceSlide.addedDate = Date()

        workspace.slideCount += 1
        workspace.modifiedDate = Date()

        let saveResult = CoreDataStack.shared.save(context: viewContext)
        switch saveResult {
        case .success:
            loadWorkspaceSlides()
        case .failure(let error):
            print("Failed to add slide to workspace: \(error)")
        }
    }
}

struct EmptyWorkspaceView: View {
    var body: some View {
        VStack(spacing: 20) {
            // Dashed border visual
            RoundedRectangle(cornerRadius: 16)
                .strokeBorder(
                    style: StrokeStyle(lineWidth: 2, dash: [10, 5])
                )
                .foregroundColor(Color.brandPrimary.opacity(0.3))
                .frame(width: 300, height: 200)
                .overlay(
                    VStack(spacing: 16) {
                        Image(systemName: "square.and.arrow.down.fill")
                            .font(.system(size: 48))
                            .foregroundColor(.brandPrimary.opacity(0.6))

                        VStack(spacing: 8) {
                            Text("Drop Slides Here")
                                .font(.system(size: 18, weight: .semibold))
                                .foregroundColor(.primary)

                            Text("Drag slides from the left to build your presentation")
                                .font(.system(size: 13))
                                .foregroundColor(.secondary)
                                .multilineTextAlignment(.center)
                                .frame(maxWidth: 250)
                        }
                    }
                )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.backgroundPrimary)
    }
}
