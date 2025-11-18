//
//  WorkspaceView.swift
//  Slideflow
//
//  List of all workspaces with create/delete functionality
//

import SwiftUI
import CoreData

struct WorkspaceView: View {
    @Environment(\.managedObjectContext) private var viewContext
    @FetchRequest(
        sortDescriptors: [NSSortDescriptor(keyPath: \Workspace.modifiedDate, ascending: false)],
        animation: .default)
    private var workspaces: FetchedResults<Workspace>

    @State private var showingCreateWorkspace = false
    @State private var newWorkspaceName = ""
    @State private var selectedWorkspace: Workspace?
    @State private var showingDeleteAlert = false

    var body: some View {
        NavigationView {
            List {
                ForEach(workspaces) { workspace in
                    NavigationLink(destination: WorkspaceEditorView(workspace: workspace)) {
                        WorkspaceRow(workspace: workspace)
                    }
                    .swipeActions(edge: .trailing) {
                        Button(role: .destructive) {
                            selectedWorkspace = workspace
                            showingDeleteAlert = true
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                    }
                }
            }
            .navigationTitle("Workspaces")
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button(action: { showingCreateWorkspace = true }) {
                        Label("New Workspace", systemImage: "square.grid.2x2")
                    }
                    .keyboardShortcut("n", modifiers: .command)
                }
            }
            .sheet(isPresented: $showingCreateWorkspace) {
                CreateWorkspaceView(isPresented: $showingCreateWorkspace)
            }
            .alert("Delete Workspace", isPresented: $showingDeleteAlert, presenting: selectedWorkspace) { workspace in
                Button("Cancel", role: .cancel) {}
                Button("Delete", role: .destructive) {
                    deleteWorkspace(workspace)
                }
            } message: { workspace in
                Text("Are you sure you want to delete '\(workspace.name)'? This will remove all slides from the workspace.")
            }
        }
    }

    private func deleteWorkspace(_ workspace: Workspace) {
        withAnimation {
            viewContext.delete(workspace)
            _ = CoreDataStack.shared.save(context: viewContext)
        }
    }
}

struct WorkspaceRow: View {
    @ObservedObject var workspace: Workspace

    var body: some View {
        HStack {
            Image(systemName: "square.grid.2x2")
                .foregroundColor(.blue)

            VStack(alignment: .leading, spacing: 4) {
                Text(workspace.name)
                    .font(.headline)

                HStack {
                    Text("\(workspace.slideCount) slides")
                        .font(.caption)
                        .foregroundColor(.secondary)

                    Text("·")
                        .foregroundColor(.secondary)

                    Text("Modified \(workspace.modifiedDate, style: .relative)")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }

            Spacer()

            if workspace.isActive {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundColor(.green)
            }
        }
        .padding(.vertical, 4)
    }
}

struct CreateWorkspaceView: View {
    @Environment(\.managedObjectContext) private var viewContext
    @Binding var isPresented: Bool

    @State private var workspaceName: String = ""
    @State private var errorMessage: String?

    var body: some View {
        VStack(spacing: 20) {
            Text("Create New Workspace")
                .font(.headline)

            TextField("Workspace Name", text: $workspaceName)
                .textFieldStyle(.roundedBorder)
                .onSubmit {
                    createWorkspace()
                }

            if let error = errorMessage {
                Text(error)
                    .foregroundColor(.red)
                    .font(.caption)
            }

            HStack {
                Button("Cancel") {
                    isPresented = false
                }
                .keyboardShortcut(.cancelAction)

                Spacer()

                Button("Create") {
                    createWorkspace()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(workspaceName.isEmpty)
            }
        }
        .padding()
        .frame(width: 400, height: 200)
    }

    private func createWorkspace() {
        guard !workspaceName.isEmpty else { return }

        // Check for duplicate name
        let fetchRequest: NSFetchRequest<Workspace> = Workspace.fetchRequest()
        fetchRequest.predicate = NSPredicate(format: "name == %@", workspaceName)

        do {
            let existing = try viewContext.fetch(fetchRequest)
            if !existing.isEmpty {
                errorMessage = "A workspace with this name already exists"
                return
            }
        } catch {
            errorMessage = "Failed to check for duplicates"
            return
        }

        // Create workspace
        let workspace = Workspace(context: viewContext)
        workspace.workspaceId = UUID()
        workspace.name = workspaceName
        workspace.createdDate = Date()
        workspace.modifiedDate = Date()
        workspace.slideCount = 0
        workspace.isActive = true

        let saveResult = CoreDataStack.shared.save(context: viewContext)
        switch saveResult {
        case .success:
            isPresented = false
        case .failure(let error):
            errorMessage = "Failed to save: \(error.localizedDescription)"
        }
    }
}
