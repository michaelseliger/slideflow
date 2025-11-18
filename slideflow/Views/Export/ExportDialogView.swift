//
//  ExportDialogView.swift
//  Slideflow
//
//  Export dialog with file location picker and progress tracking
//

import SwiftUI
import AppKit
import UniformTypeIdentifiers

struct ExportDialogView: View {
    let workspace: Workspace
    @Environment(\.dismiss) private var dismiss

    @State private var selectedPath: String?
    @State private var isExporting: Bool = false
    @State private var exportProgress: Double = 0.0
    @State private var errorMessage: String?
    @State private var exportSuccess: Bool = false
    @State private var exportedFilePath: String?

    private let exporter = PowerPointExporter()

    var body: some View {
        VStack(spacing: 24) {
            if !exportSuccess {
                // Icon and title
                VStack(spacing: 8) {
                    Image(systemName: "square.and.arrow.up.fill")
                        .font(.system(size: 48))
                        .foregroundColor(.brandPrimary)

                    Text("Export Presentation")
                        .font(.title2.bold())

                    HStack(spacing: 4) {
                        Text("\(workspace.slideCount) slides")
                            .font(.subheadline)
                            .foregroundColor(.secondary)
                        Text("from")
                            .font(.subheadline)
                            .foregroundColor(.secondary)
                        Text("\"\(workspace.name)\"")
                            .font(.subheadline)
                            .fontWeight(.semibold)
                            .foregroundColor(.primary)
                    }
                }

                // Export location
                if let path = selectedPath {
                    ModernCard {
                        HStack(spacing: 12) {
                            Image(systemName: "doc.fill")
                                .foregroundColor(.brandPrimary)
                                .font(.system(size: 20))

                            VStack(alignment: .leading, spacing: 4) {
                                Text("Export Location")
                                    .font(.caption)
                                    .fontWeight(.semibold)
                                    .foregroundColor(.secondary)

                                Text(path)
                                    .font(.body)
                                    .lineLimit(2)
                                    .truncationMode(.middle)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }

                // Choose location button
                PrimaryButton("Choose Location...", icon: "folder") {
                    selectOutputLocation()
                }
                .disabled(isExporting)

                // Large workspace warning
                if workspace.slideCount > 100 {
                    HStack(spacing: 8) {
                        Image(systemName: "clock.fill")
                            .font(.caption)
                        Text("Large deck - export may take several minutes")
                            .font(.caption)
                    }
                    .foregroundColor(.orange)
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.orange.opacity(0.08))
                    .cornerRadius(8)
                }

                // Progress indicator
                if isExporting {
                    VStack(spacing: 12) {
                        ProgressView(value: exportProgress, total: 1.0)
                            .tint(.brandPrimary)

                        HStack(spacing: 8) {
                            ProgressView()
                                .scaleEffect(0.7)
                            Text("Exporting... \(Int(exportProgress * 100))%")
                                .font(.subheadline)
                                .foregroundColor(.secondary)
                        }
                    }
                    .padding(16)
                    .background(Color.backgroundSubtle)
                    .cornerRadius(10)
                }

                // Error message
                if let error = errorMessage {
                    HStack(spacing: 8) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(.caption)
                        Text(error)
                            .font(.caption)
                    }
                    .foregroundColor(.red)
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.red.opacity(0.08))
                    .cornerRadius(8)
                }

                Divider()

                // Actions
                HStack(spacing: 12) {
                    SecondaryButton("Cancel") {
                        dismiss()
                    }
                    .keyboardShortcut(.cancelAction)
                    .disabled(isExporting)

                    Spacer()

                    PrimaryButton("Export Presentation", icon: "square.and.arrow.up") {
                        performExport()
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(selectedPath == nil || isExporting)
                }
            } else {
                // Success state
                VStack(spacing: 20) {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 64))
                        .foregroundColor(.brandSuccess)

                    VStack(spacing: 8) {
                        Text("Export Successful!")
                            .font(.title2.bold())

                        Text("Your presentation has been created")
                            .font(.subheadline)
                            .foregroundColor(.secondary)
                    }

                    if let filePath = exportedFilePath {
                        SecondaryButton("Show in Finder", icon: "folder") {
                            showInFinder(path: filePath)
                        }
                        .padding(.top, 8)
                    }

                    Divider()
                        .padding(.vertical, 8)

                    PrimaryButton("Done", icon: "checkmark") {
                        dismiss()
                    }
                    .keyboardShortcut(.defaultAction)
                }
                .padding(.vertical, 20)
            }
        }
        .padding(32)
        .frame(width: 560)
        .background(Color.backgroundCard)
    }

    private func selectOutputLocation() {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.init(filenameExtension: "pptx")!]
        panel.nameFieldStringValue = "\(workspace.name).pptx"
        panel.prompt = "Export"

        panel.begin { response in
            if response == .OK, let url = panel.url {
                selectedPath = url.path
            }
        }
    }

    private func performExport() {
        guard let outputPath = selectedPath else { return }

        isExporting = true
        exportProgress = 0.0

        Task {
            let result = await exporter.exportWorkspace(
                workspaceId: workspace.workspaceId,
                outputPath: outputPath,
                progressCallback: { progress in
                    DispatchQueue.main.async {
                        exportProgress = progress
                    }
                }
            )

            await MainActor.run {
                isExporting = false

                switch result {
                case .success(let filePath):
                    exportSuccess = true
                    exportedFilePath = filePath
                case .failure(let error):
                    errorMessage = "Export failed: \(error.localizedDescription)"
                    print("❌ Export error details: \(error)")
                }
            }
        }
    }

    private func showInFinder(path: String) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }
}
