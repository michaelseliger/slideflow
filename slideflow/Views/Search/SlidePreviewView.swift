//
//  SlidePreviewView.swift
//  Slideflow
//
//  Full-size slide preview with source presentation info
//

import SwiftUI
import AppKit

struct SlidePreviewView: View {
    let slide: IndexedSlide
    @Environment(\.dismiss) private var dismiss
    @State private var previewImage: NSImage?

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Slide \(slide.slideNumber)")
                        .font(.headline)

                    Text(slide.sourceFileName)
                        .font(.subheadline)
                        .foregroundColor(.secondary)

                    Text(slide.sourceDirectory)
                        .font(.caption)
                        .foregroundColor(.secondary)
                }

                Spacer()

                Button("Close") {
                    dismiss()
                }
            }
            .padding()
            .background(Color.secondary.opacity(0.1))

            Divider()

            // Preview image
            GeometryReader { geometry in
                if let image = previewImage {
                    Image(nsImage: image)
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                        .frame(width: geometry.size.width, height: geometry.size.height)
                } else {
                    ProgressView("Loading preview...")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }

            Divider()

            // Slide content
            if !slide.textContent.isEmpty {
                ScrollView {
                    Text(slide.textContent)
                        .font(.body)
                        .padding()
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 150)
                .background(Color.secondary.opacity(0.05))
            }

            // Metadata
            HStack(spacing: 16) {
                if slide.hasImages {
                    Label("Has Images", systemImage: "photo")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }

                if slide.hasMedia {
                    Label("Has Media", systemImage: "play.circle")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }

                Spacer()

                Text("Indexed: \(slide.extractedDate, style: .relative)")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
            .padding()
            .background(Color.secondary.opacity(0.1))
        }
        .frame(width: 800, height: 700)
        .onAppear {
            loadPreview()
        }
    }

    private func loadPreview() {
        DispatchQueue.global(qos: .userInitiated).async {
            if let image = NSImage(contentsOfFile: slide.thumbnailPath) {
                DispatchQueue.main.async {
                    previewImage = image
                }
            }
        }
    }
}
