//
//  SearchResultsView.swift
//  Slideflow
//
//  Grid view of slide thumbnails with lazy loading
//

import SwiftUI
import AppKit

struct SearchResultsView: View {
    let slides: [IndexedSlide]
    @State private var selectedSlide: IndexedSlide?

    private let columns = [
        GridItem(.adaptive(minimum: 180, maximum: 220), spacing: 16)
    ]

    var body: some View {
        ScrollView {
            LazyVGrid(columns: columns, spacing: 16) {
                ForEach(slides) { slide in
                    SlideCard(slide: slide)
                        .onTapGesture {
                            selectedSlide = slide
                        }
                        .onDrag {
                            // Create draggable item with slide UUID as plain text
                            NSItemProvider(object: slide.slideId.uuidString as NSString)
                        }
                }
            }
            .padding()
        }
        .sheet(item: $selectedSlide) { slide in
            SlidePreviewView(slide: slide)
        }
    }
}

struct SlideCard: View {
    let slide: IndexedSlide
    @State private var thumbnailImage: NSImage?
    @State private var isLoading = true

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            // Thumbnail
            Group {
                if let image = thumbnailImage {
                    Image(nsImage: image)
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                } else if isLoading {
                    Rectangle()
                        .fill(Color.secondary.opacity(0.2))
                        .overlay {
                            ProgressView()
                        }
                } else {
                    // Fallback: Show text preview
                    Rectangle()
                        .fill(Color.blue.opacity(0.1))
                        .overlay {
                            VStack {
                                Image(systemName: "doc.text")
                                    .font(.system(size: 32))
                                    .foregroundColor(.blue)
                                Text("Slide \(slide.slideNumber)")
                                    .font(.caption)
                                    .foregroundColor(.secondary)
                            }
                        }
                }
            }
            .frame(height: 150)
            .frame(maxWidth: .infinity)
            .background(Color.secondary.opacity(0.1))
            .cornerRadius(8)

            // Slide info
            VStack(alignment: .leading, spacing: 4) {
                Text("Slide \(slide.slideNumber)")
                    .font(.caption)
                    .fontWeight(.semibold)

                Text(slide.sourceFileName)
                    .font(.caption2)
                    .foregroundColor(.secondary)
                    .lineLimit(1)

                if !slide.textContent.isEmpty {
                    Text(slide.textContent)
                        .font(.caption2)
                        .foregroundColor(.secondary)
                        .lineLimit(2)
                }
            }
        }
        .padding(8)
        .background(Color.secondary.opacity(0.05))
        .cornerRadius(8)
        .onAppear {
            loadThumbnail()
        }
    }

    private func loadThumbnail() {
        guard !slide.thumbnailPath.isEmpty else {
            isLoading = false
            return
        }

        DispatchQueue.global(qos: .userInitiated).async {
            if let image = NSImage(contentsOfFile: slide.thumbnailPath) {
                DispatchQueue.main.async {
                    thumbnailImage = image
                    isLoading = false
                }
            } else {
                DispatchQueue.main.async {
                    isLoading = false
                }
            }
        }
    }
}
