//
//  SearchView.swift
//  Slideflow
//
//  Search interface with query input, filters, and keyboard shortcuts
//

import SwiftUI
import CoreData

struct SearchView: View {
    @Environment(\.managedObjectContext) private var viewContext

    @State private var searchQuery: String = ""
    @State private var searchResults: [IndexedSlide] = []
    @State private var isSearching: Bool = false
    @State private var resultCount: Int = 0

    // Filters
    @State private var selectedPresentationFilter: String?
    @State private var selectedDirectoryFilter: String?
    @State private var dateRangeStart: Date?
    @State private var dateRangeEnd: Date?
    @State private var showingFilters: Bool = false

    private let searchEngine = SearchEngine()
    private let searchFilter = SearchFilter()

    var body: some View {
        NavigationView {
            VStack(spacing: 0) {
                // Search bar
                HStack {
                    Image(systemName: "magnifyingglass")
                        .foregroundColor(.secondary)

                    TextField("Search slides...", text: $searchQuery)
                        .textFieldStyle(.plain)
                        .onSubmit {
                            performSearch()
                        }

                    if !searchQuery.isEmpty {
                        Button(action: { searchQuery = "" }) {
                            Image(systemName: "xmark.circle.fill")
                                .foregroundColor(.secondary)
                        }
                        .buttonStyle(.plain)
                    }

                    Button(action: { performSearch() }) {
                        Text("Search")
                    }
                    .keyboardShortcut(.return, modifiers: [])
                }
                .padding()
                .background(Color.secondary.opacity(0.1))

                // Filter controls
                if showingFilters {
                    FilterControlsView(
                        selectedPresentation: $selectedPresentationFilter,
                        selectedDirectory: $selectedDirectoryFilter,
                        dateStart: $dateRangeStart,
                        dateEnd: $dateRangeEnd
                    )
                    .padding()
                }

                // Result count
                if resultCount > 0 {
                    HStack {
                        Text("Found \(resultCount) slides")
                            .font(.caption)
                            .foregroundColor(.secondary)
                        Spacer()
                    }
                    .padding(.horizontal)
                }

                Divider()

                // Results
                if isSearching {
                    ProgressView("Searching...")
                        .padding()
                } else if searchResults.isEmpty && !searchQuery.isEmpty {
                    EmptyResultsView()
                } else {
                    SearchResultsView(slides: searchResults)
                }
            }
            .navigationTitle("Search Slides")
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button(action: { showingFilters.toggle() }) {
                        Label("Filters", systemImage: "line.3.horizontal.decrease.circle")
                    }
                }
            }
        }
    }

    private func performSearch() {
        isSearching = true

        Task {
            var filters: [Filter] = []

            if let presentation = selectedPresentationFilter {
                filters.append(PresentationNameFilter(name: presentation))
            }

            if let directory = selectedDirectoryFilter {
                filters.append(FileLocationFilter(directoryPath: directory))
            }

            if let start = dateRangeStart, let end = dateRangeEnd {
                filters.append(DateRangeFilter(start: start, end: end))
            }

            let result: Result<[IndexedSlide], CoreDataError>
            if filters.isEmpty {
                result = await searchEngine.search(query: searchQuery, in: viewContext)
            } else {
                result = await searchFilter.searchWithFilters(query: searchQuery, filters: filters, in: viewContext)
            }

            await MainActor.run {
                switch result {
                case .success(let slides):
                    searchResults = slides
                    resultCount = slides.count
                case .failure:
                    searchResults = []
                    resultCount = 0
                }
                isSearching = false
            }
        }
    }
}

struct FilterControlsView: View {
    @Binding var selectedPresentation: String?
    @Binding var selectedDirectory: String?
    @Binding var dateStart: Date?
    @Binding var dateEnd: Date?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Filters")
                .font(.headline)

            HStack {
                VStack(alignment: .leading) {
                    Text("Presentation")
                        .font(.caption)
                    TextField("Filter by name...", text: Binding(
                        get: { selectedPresentation ?? "" },
                        set: { selectedPresentation = $0.isEmpty ? nil : $0 }
                    ))
                    .textFieldStyle(.roundedBorder)
                }

                VStack(alignment: .leading) {
                    Text("Directory")
                        .font(.caption)
                    TextField("Filter by path...", text: Binding(
                        get: { selectedDirectory ?? "" },
                        set: { selectedDirectory = $0.isEmpty ? nil : $0 }
                    ))
                    .textFieldStyle(.roundedBorder)
                }
            }

            HStack {
                DatePicker("From", selection: Binding(
                    get: { dateStart ?? Date() },
                    set: { dateStart = $0 }
                ), displayedComponents: .date)
                .labelsHidden()

                DatePicker("To", selection: Binding(
                    get: { dateEnd ?? Date() },
                    set: { dateEnd = $0 }
                ), displayedComponents: .date)
                .labelsHidden()
            }
        }
        .padding()
        .background(Color.secondary.opacity(0.05))
        .cornerRadius(8)
    }
}

struct EmptyResultsView: View {
    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 48))
                .foregroundColor(.secondary)

            Text("No slides found matching your search")
                .font(.headline)
                .foregroundColor(.secondary)

            Text("Try different keywords or adjust your filters")
                .font(.caption)
                .foregroundColor(.secondary)
        }
        .padding()
    }
}
