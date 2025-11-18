//
//  SlideReorderView.swift
//  Slideflow
//
//  Drag-and-drop reordering of workspace slides
//

import SwiftUI
import AppKit

struct SlideReorderView: View {
    @Binding var workspaceSlides: [WorkspaceSlide]
    let onReorder: (IndexSet, Int) -> Void
    let onDelete: (IndexSet) -> Void

    @State private var thumbnailCache: [UUID: NSImage] = [:]

    var body: some View {
        List {
            ForEach(Array(workspaceSlides.enumerated()), id: \.element.id) { index, workspaceSlide in
                if let slide = workspaceSlide.slide {
                    WorkspaceSlideRow(
                        slide: slide,
                        orderIndex: index + 1,
                        thumbnail: thumbnailCache[slide.slideId],
                        onThumbnailLoad: { image in
                            thumbnailCache[slide.slideId] = image
                        }
                    )
                } else {
                    NullSlideRow()
                }
            }
            .onMove { source, destination in
                onReorder(source, destination)
            }
            .onDelete { indexSet in
                onDelete(indexSet)
            }
        }
    }
}

struct WorkspaceSlideRow: View {
    let slide: IndexedSlide
    let orderIndex: Int
    let thumbnail: NSImage?
    let onThumbnailLoad: (NSImage) -> Void

    var body: some View {
        HStack(spacing: 12) {
            // Order number
            Text("\(orderIndex)")
                .font(.headline)
                .foregroundColor(.secondary)
                .frame(width: 30)

            // Thumbnail
            Group {
                if let image = thumbnail {
                    Image(nsImage: image)
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                } else {
                    Rectangle()
                        .fill(Color.secondary.opacity(0.2))
                        .overlay {
                            ProgressView()
                                .scaleEffect(0.5)
                        }
                }
            }
            .frame(width: 80, height: 60)
            .cornerRadius(4)

            // Info
            VStack(alignment: .leading, spacing: 4) {
                Text("Slide \(slide.slideNumber)")
                    .font(.headline)

                Text(slide.sourceFileName)
                    .font(.caption)
                    .foregroundColor(.secondary)

                if !slide.textContent.isEmpty {
                    Text(slide.textContent)
                        .font(.caption2)
                        .foregroundColor(.secondary)
                        .lineLimit(1)
                }
            }

            Spacer()
        }
        .padding(.vertical, 4)
        .onAppear {
            loadThumbnail()
        }
    }

    private func loadThumbnail() {
        guard thumbnail == nil else { return }

        DispatchQueue.global(qos: .userInitiated).async {
            if let image = NSImage(contentsOfFile: slide.thumbnailPath) {
                DispatchQueue.main.async {
                    onThumbnailLoad(image)
                }
            }
        }
    }
}

struct NullSlideRow: View {
    var body: some View {
        HStack {
            Image(systemName: "exclamationmark.triangle")
                .foregroundColor(.orange)

            Text("Slide unavailable (source deleted)")
                .font(.caption)
                .foregroundColor(.secondary)
                .italic()
        }
        .padding(.vertical, 4)
    }
}
