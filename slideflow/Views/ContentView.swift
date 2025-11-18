//
//  ContentView.swift
//  Slideflow
//
//  Beautiful, modern UI for marketers
//

import SwiftUI
import CoreData

struct ContentView: View {
    @State private var selectedTab: Int = 0

    var body: some View {
        ZStack {
            // Soft gradient background
            Color.backgroundPrimary
                .ignoresSafeArea()

            TabView(selection: $selectedTab) {
                DirectoryListView()
                    .tabItem {
                        Label("My Slides", systemImage: "folder.fill")
                    }
                    .tag(0)

                SearchAndWorkspaceView()
                    .tabItem {
                        Label("Create Deck", systemImage: "sparkles")
                    }
                    .tag(1)
            }
        }
    }
}

/// Card component for individual decks
struct DeckCard: View {
    let workspace: Workspace
    let isSelected: Bool
    let onSelect: () -> Void
    let onDelete: () -> Void

    var body: some View {
        ModernCard {
            VStack(alignment: .leading, spacing: 16) {
                // Header: Icon + Name + Delete button
                HStack(spacing: 12) {
                    Image(systemName: "square.grid.2x2.fill")
                        .font(.system(size: 32))
                        .foregroundColor(.brandPrimary)

                    VStack(alignment: .leading, spacing: 4) {
                        Text(workspace.name)
                            .font(.system(size: 16, weight: .semibold))
                            .foregroundColor(.textPrimary)

                        Text("Modified \(workspace.modifiedDate, style: .relative)")
                            .font(.caption)
                            .foregroundColor(.textSecondary)
                    }

                    Spacer()

                    Button(action: onDelete) {
                        Image(systemName: "trash")
                            .foregroundColor(.red.opacity(0.8))
                            .font(.system(size: 16))
                    }
                    .buttonStyle(.plain)
                    .help("Delete deck")
                }

                Divider()

                // Stats: Slide count
                HStack(spacing: 8) {
                    Image(systemName: "doc.on.doc.fill")
                        .foregroundColor(.brandPrimary.opacity(0.7))
                        .font(.system(size: 14))

                    VStack(alignment: .leading, spacing: 2) {
                        Text("\(workspace.slideCount)")
                            .font(.system(size: 18, weight: .bold))
                            .foregroundColor(.textPrimary)

                        Text("slides")
                            .font(.caption)
                            .foregroundColor(.textSecondary)
                    }
                }
            }
            .padding(20)
        }
        .onTapGesture {
            onSelect()
        }
        .overlay(
            isSelected ?
                RoundedRectangle(cornerRadius: 16)
                    .stroke(Color.brandPrimary, lineWidth: 2)
                : nil
        )
    }
}

/// Modern split view for creating presentations
struct SearchAndWorkspaceView: View {
    @Environment(\.managedObjectContext) private var viewContext
    @FetchRequest(
        sortDescriptors: [NSSortDescriptor(keyPath: \Workspace.modifiedDate, ascending: false)],
        animation: .default)
    private var workspaces: FetchedResults<Workspace>

    @State private var selectedWorkspace: Workspace?
    @State private var showingCreateWorkspace = false
    @State private var selectedDeckToDelete: Workspace?
    @State private var showingDeleteConfirmation = false

    var body: some View {
        HSplitView {
            // Left: Search & Browse
            VStack(spacing: 0) {
                // Header
                VStack(alignment: .leading, spacing: 8) {
                    Text("Find Your Slides")
                        .font(.system(size: 24, weight: .bold))
                        .foregroundColor(.textPrimary)

                    Text("Search through all your PowerPoint presentations")
                        .font(.system(size: 14))
                        .foregroundColor(.textSecondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(24)
                .background(Color.backgroundCard)
                .overlay(
                    Rectangle()
                        .fill(Color.borderLight)
                        .frame(height: 1),
                    alignment: .bottom
                )

                SearchView()
            }
            .frame(minWidth: 450, maxWidth: 600)

            // Right: Workspace
            VStack(spacing: 0) {
                // Workspace header
                VStack(spacing: 16) {
                    HStack {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Your Presentation")
                                .font(.system(size: 20, weight: .bold))
                                .foregroundColor(.textPrimary)

                            Text("Drag slides from the left to build your deck")
                                .font(.system(size: 13))
                                .foregroundColor(.textSecondary)
                        }

                        Spacer()

                        PrimaryButton("New Deck", icon: "plus.circle.fill") {
                            showingCreateWorkspace = true
                        }
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

                // Deck cards
                if !workspaces.isEmpty {
                    ScrollView {
                        VStack(spacing: 16) {
                            ForEach(workspaces) { workspace in
                                DeckCard(
                                    workspace: workspace,
                                    isSelected: selectedWorkspace == workspace,
                                    onSelect: {
                                        selectedWorkspace = workspace
                                    },
                                    onDelete: {
                                        selectedDeckToDelete = workspace
                                        showingDeleteConfirmation = true
                                    }
                                )
                            }
                        }
                        .padding(24)
                    }
                    .background(Color.backgroundPrimary)
                }

                // Workspace content
                if let workspace = selectedWorkspace {
                    WorkspaceEditorView(workspace: workspace)
                } else {
                    EmptyStateView(
                        icon: "sparkles",
                        title: workspaces.isEmpty ? "Create Your First Deck" : "Select a Deck to Start",
                        message: workspaces.isEmpty ?
                            "Build beautiful presentations by combining slides from different PowerPoint files" :
                            "Choose a deck from the dropdown above to start adding slides",
                        actionTitle: workspaces.isEmpty ? "Create New Deck" : nil,
                        action: workspaces.isEmpty ? { showingCreateWorkspace = true } : nil
                    )
                }
            }
            .frame(minWidth: 500)
            .background(Color.backgroundPrimary)
        }
        .sheet(isPresented: $showingCreateWorkspace) {
            CreateWorkspaceView(isPresented: $showingCreateWorkspace)
        }
        .alert("Delete Deck", isPresented: $showingDeleteConfirmation, presenting: selectedDeckToDelete) { deck in
            Button("Cancel", role: .cancel) {}
            Button("Delete", role: .destructive) {
                deleteDeck(deck)
            }
        } message: { deck in
            Text("Are you sure you want to delete '\(deck.name)'? This will remove all slides from the deck.")
        }
        .onAppear {
            if selectedWorkspace == nil && !workspaces.isEmpty {
                selectedWorkspace = workspaces.first
            }
        }
    }

    private func deleteDeck(_ workspace: Workspace) {
        // If deleting the selected workspace, clear selection
        if selectedWorkspace == workspace {
            selectedWorkspace = nil
        }

        // Delete the workspace
        viewContext.delete(workspace)

        do {
            try viewContext.save()
        } catch {
            print("Error deleting workspace: \(error)")
        }
    }
}

#Preview {
    ContentView()
        .environment(\.managedObjectContext, CoreDataStack.shared.viewContext)
}
