//
//  IndexedSlide.swift
//  Slideflow
//
//  NSManagedObject subclass for IndexedSlide entity
//

import Foundation
import CoreData

@objc(IndexedSlide)
public class IndexedSlide: NSManagedObject {
    @NSManaged public var slideId: UUID
    @NSManaged public var slideNumber: Int16
    @NSManaged public var textContent: String
    @NSManaged public var thumbnailPath: String
    @NSManaged public var thumbnailWidth: Int16
    @NSManaged public var thumbnailHeight: Int16
    @NSManaged public var hasImages: Bool
    @NSManaged public var hasMedia: Bool
    @NSManaged public var slideLayout: String?
    @NSManaged public var extractedDate: Date
    @NSManaged public var searchRank: Double
    @NSManaged public var sourcePresentation: SourcePresentation?
    @NSManaged public var workspaceSlides: NSSet?

    /// Computed properties
    var sourceFileName: String {
        sourcePresentation?.filename ?? "Unknown"
    }

    var sourceFilePath: String {
        sourcePresentation?.filePath ?? ""
    }

    var sourceDirectory: String {
        sourcePresentation?.directory?.path ?? ""
    }

    /// Fetch request helper
    @nonobjc public class func fetchRequest() -> NSFetchRequest<IndexedSlide> {
        return NSFetchRequest<IndexedSlide>(entityName: "IndexedSlide")
    }
}

// MARK: - Generated accessors for workspaceSlides
extension IndexedSlide {
    @objc(addWorkspaceSlidesObject:)
    @NSManaged public func addToWorkspaceSlides(_ value: WorkspaceSlide)

    @objc(removeWorkspaceSlidesObject:)
    @NSManaged public func removeFromWorkspaceSlides(_ value: WorkspaceSlide)

    @objc(addWorkspaceSlides:)
    @NSManaged public func addToWorkspaceSlides(_ values: NSSet)

    @objc(removeWorkspaceSlides:)
    @NSManaged public func removeFromWorkspaceSlides(_ values: NSSet)
}

extension IndexedSlide: Identifiable {
    public var id: UUID { slideId }
}
