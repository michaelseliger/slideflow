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

/// Modern split view for creating presentations
struct SearchAndWorkspaceView: View {
    @Environment(\.managedObjectContext) private var viewContext
    @FetchRequest(
        sortDescriptors: [NSSortDescriptor(keyPath: \Workspace.modifiedDate, ascending: false)],
        animation: .default)
    private var workspaces: FetchedResults<Workspace>

    @State private var selectedWorkspace: Workspace?
    @State private var showingCreateWorkspace = false

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
                .background(
                    LinearGradient(
                        colors: [Color.brandPrimary.opacity(0.08), Color.brandPrimary.opacity(0.02)],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    )
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

                    // Workspace picker
                    if !workspaces.isEmpty {
                        HStack(spacing: 12) {
                            Image(systemName: "folder.fill")
                                .foregroundColor(.brandPrimary)
                                .font(.system(size: 16))

                            Picker("", selection: $selectedWorkspace) {
                                Text("Select a deck...").tag(nil as Workspace?)
                                ForEach(workspaces) { workspace in
                                    HStack {
                                        Text(workspace.name)
                                        Spacer()
                                        Badge(
                                            text: "\(workspace.slideCount) slides",
                                            color: .brandPrimary
                                        )
                                    }
                                    .tag(workspace as Workspace?)
                                }
                            }
                            .labelsHidden()
                            .frame(maxWidth: .infinity)
                        }
                        .padding(12)
                        .background(Color.backgroundSubtle)
                        .cornerRadius(10)
                    }
                }
                .padding(24)
                .background(Color.white)
                .overlay(
                    Rectangle()
                        .fill(Color.borderLight)
                        .frame(height: 1),
                    alignment: .bottom
                )

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
        .onAppear {
            if selectedWorkspace == nil && !workspaces.isEmpty {
                selectedWorkspace = workspaces.first
            }
        }
    }
}

#Preview {
    ContentView()
        .environment(\.managedObjectContext, CoreDataStack.shared.viewContext)
}
