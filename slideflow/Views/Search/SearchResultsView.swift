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

    // Use flexible 2-column layout for better space usage
    private let columns = [
        GridItem(.flexible(minimum: 200, maximum: 350), spacing: 20),
        GridItem(.flexible(minimum: 200, maximum: 350), spacing: 20)
    ]

    var body: some View {
        ScrollView {
            LazyVGrid(columns: columns, spacing: 20) {
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
            .padding(24)
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
                        .fill(Color.backgroundSubtle)
                        .overlay {
                            ProgressView()
                        }
                } else {
                    // Fallback: Show text preview
                    Rectangle()
                        .fill(Color.brandPrimary.opacity(0.08))
                        .overlay {
                            VStack {
                                Image(systemName: "doc.text")
                                    .font(.system(size: 32))
                                    .foregroundColor(.brandPrimary)
                                Text("Slide \(slide.slideNumber)")
                                    .font(.caption)
                                    .foregroundColor(.secondary)
                            }
                        }
                }
            }
            .frame(height: 150)
            .frame(maxWidth: .infinity)
            .background(Color.backgroundCard)
            .cornerRadius(8)
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(Color.borderLight, lineWidth: 0.5)
            )

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
        .padding(12)
        .background(Color.backgroundCard)
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color.borderLight, lineWidth: 0.5)
        )
        .shadow(color: Color.black.opacity(0.02), radius: 4, x: 0, y: 2)
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
