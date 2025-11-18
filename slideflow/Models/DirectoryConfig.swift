//
//  DirectoryConfig.swift
//  Slideflow
//
//  NSManagedObject subclass for DirectoryConfig entity
//

import Foundation
import CoreData

@objc(DirectoryConfig)
public class DirectoryConfig: NSManagedObject {
    @NSManaged public var directoryId: UUID
    @NSManaged public var path: String
    @NSManaged public var securityBookmark: Data? // Security bookmark for directory access
    @NSManaged public var isActive: Bool
    @NSManaged public var lastScanDate: Date?
    @NSManaged public var addedDate: Date
    @NSManaged public var slideCount: Int32
    @NSManaged public var presentations: NSSet?

    /// Validation: Path must be absolute and exist
    public override func validateForInsert() throws {
        try super.validateForInsert()
        try validatePath()
    }

    public override func validateForUpdate() throws {
        try super.validateForUpdate()
        try validatePath()
    }

    private func validatePath() throws {
        guard !path.isEmpty else {
            throw CoreDataError.validationFailed(message: "Path cannot be empty")
        }

        guard path.hasPrefix("/") else {
            throw CoreDataError.validationFailed(message: "Path must be absolute")
        }

        guard FileManager.default.fileExists(atPath: path) else {
            throw CoreDataError.validationFailed(message: "Path does not exist")
        }
    }
}

// MARK: - Generated accessors for presentations
extension DirectoryConfig {
    @objc(addPresentationsObject:)
    @NSManaged public func addToPresentations(_ value: SourcePresentation)

    @objc(removePresentationsObject:)
    @NSManaged public func removeFromPresentations(_ value: SourcePresentation)

    @objc(addPresentations:)
    @NSManaged public func addToPresentations(_ values: NSSet)

    @objc(removePresentations:)
    @NSManaged public func removeFromPresentations(_ values: NSSet)
}

extension DirectoryConfig: Identifiable {
    public var id: UUID { directoryId }

    @nonobjc public class func fetchRequest() -> NSFetchRequest<DirectoryConfig> {
        return NSFetchRequest<DirectoryConfig>(entityName: "DirectoryConfig")
    }
}
