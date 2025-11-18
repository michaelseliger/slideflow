//
//  SourcePresentation.swift
//  Slideflow
//
//  NSManagedObject subclass for SourcePresentation entity
//

import Foundation
import CoreData

@objc(SourcePresentation)
public class SourcePresentation: NSManagedObject {
    @NSManaged public var presentationId: UUID
    @NSManaged public var filename: String
    @NSManaged public var filePath: String?
    @NSManaged public var securityBookmark: Data? // Security-scoped bookmark for sandbox access
    @NSManaged public var fileSize: Int64
    @NSManaged public var fileModifiedDate: Date
    @NSManaged public var fileCreatedDate: Date
    @NSManaged public var totalSlideCount: Int16
    @NSManaged public var indexedDate: Date
    @NSManaged public var pdfCachePath: String?
    @NSManaged public var indexingStatus: String
    @NSManaged public var errorMessage: String?
    @NSManaged public var directory: DirectoryConfig?
    @NSManaged public var slides: NSSet?

    /// Computed property for sorting/filtering
    var isPending: Bool {
        indexingStatus == "pending"
    }

    var isIndexing: Bool {
        indexingStatus == "indexing"
    }

    var isCompleted: Bool {
        indexingStatus == "completed"
    }

    var hasFailed: Bool {
        indexingStatus == "failed"
    }
}

// MARK: - Generated accessors for slides
extension SourcePresentation {
    @objc(addSlidesObject:)
    @NSManaged public func addToSlides(_ value: IndexedSlide)

    @objc(removeSlidesObject:)
    @NSManaged public func removeFromSlides(_ value: IndexedSlide)

    @objc(addSlides:)
    @NSManaged public func addToSlides(_ values: NSSet)

    @objc(removeSlides:)
    @NSManaged public func removeFromSlides(_ values: NSSet)
}

extension SourcePresentation: Identifiable {
    public var id: UUID { presentationId }

    @nonobjc public class func fetchRequest() -> NSFetchRequest<SourcePresentation> {
        return NSFetchRequest<SourcePresentation>(entityName: "SourcePresentation")
    }
}
