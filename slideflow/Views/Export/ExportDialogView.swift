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
        VStack(spacing: 20) {
            Text("Export Workspace")
                .font(.headline)

            if !exportSuccess {
                VStack(alignment: .leading, spacing: 12) {
                    Text("Workspace: \(workspace.name)")
                        .font(.subheadline)

                    Text("\(workspace.slideCount) slides will be exported")
                        .font(.caption)
                        .foregroundColor(.secondary)

                    if let path = selectedPath {
                        HStack {
                            Image(systemName: "doc.fill")
                            Text(path)
                                .font(.caption)
                                .lineLimit(1)
                        }
                        .padding(8)
                        .background(Color.secondary.opacity(0.1))
                        .cornerRadius(4)
                    }

                    Button("Choose Location...") {
                        selectOutputLocation()
                    }
                    .disabled(isExporting)

                    if workspace.slideCount > 100 {
                        HStack {
                            Image(systemName: "exclamationmark.triangle")
                                .foregroundColor(.orange)
                            Text("Large workspace - export may take several minutes")
                                .font(.caption)
                                .foregroundColor(.orange)
                        }
                    }
                }

                if isExporting {
                    VStack(spacing: 8) {
                        ProgressView(value: exportProgress, total: 1.0)

                        Text("Exporting... \(Int(exportProgress * 100))%")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                    .padding()
                }

                if let error = errorMessage {
                    Text(error)
                        .foregroundColor(.red)
                        .font(.caption)
                }

                HStack {
                    Button("Cancel") {
                        dismiss()
                    }
                    .keyboardShortcut(.cancelAction)
                    .disabled(isExporting)

                    Spacer()

                    Button("Export") {
                        performExport()
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(selectedPath == nil || isExporting)
                }
            } else {
                VStack(spacing: 16) {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 48))
                        .foregroundColor(.green)

                    Text("Export Successful")
                        .font(.headline)

                    if let filePath = exportedFilePath {
                        Button("Show in Finder") {
                            showInFinder(path: filePath)
                        }
                    }

                    Button("Done") {
                        dismiss()
                    }
                    .keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding()
        .frame(width: 500, height: exportSuccess ? 300 : 400)
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
