//
//  Workspace.swift
//  Slideflow
//
//  NSManagedObject subclass for Workspace entity
//

import Foundation
import CoreData

@objc(Workspace)
public class Workspace: NSManagedObject {
    @NSManaged public var workspaceId: UUID
    @NSManaged public var name: String
    @NSManaged public var createdDate: Date
    @NSManaged public var modifiedDate: Date
    @NSManaged public var slideCount: Int32
    @NSManaged public var isActive: Bool
    @NSManaged public var workspaceSlides: NSOrderedSet?

    /// Validation
    public override func validateForInsert() throws {
        try super.validateForInsert()
        try validateName()
    }

    public override func validateForUpdate() throws {
        try super.validateForUpdate()
        try validateName()
    }

    private func validateName() throws {
        guard !name.isEmpty else {
            throw CoreDataError.validationFailed(message: "Workspace name cannot be empty")
        }

        guard name.count <= 100 else {
            throw CoreDataError.validationFailed(message: "Workspace name too long (max 100 chars)")
        }
    }

    @nonobjc public class func fetchRequest() -> NSFetchRequest<Workspace> {
        return NSFetchRequest<Workspace>(entityName: "Workspace")
    }
}

// MARK: - Generated accessors for workspaceSlides
extension Workspace {
    @objc(insertObject:inWorkspaceSlidesAtIndex:)
    @NSManaged public func insertIntoWorkspaceSlides(_ value: WorkspaceSlide, at idx: Int)

    @objc(removeObjectFromWorkspaceSlidesAtIndex:)
    @NSManaged public func removeFromWorkspaceSlides(at idx: Int)

    @objc(insertWorkspaceSlides:atIndexes:)
    @NSManaged public func insertIntoWorkspaceSlides(_ values: [WorkspaceSlide], at indexes: NSIndexSet)

    @objc(removeWorkspaceSlidesAtIndexes:)
    @NSManaged public func removeFromWorkspaceSlides(at indexes: NSIndexSet)

    @objc(replaceObjectInWorkspaceSlidesAtIndex:withObject:)
    @NSManaged public func replaceWorkspaceSlides(at idx: Int, with value: WorkspaceSlide)

    @objc(replaceWorkspaceSlidesAtIndexes:withWorkspaceSlides:)
    @NSManaged public func replaceWorkspaceSlides(at indexes: NSIndexSet, with values: [WorkspaceSlide])

    @objc(addWorkspaceSlidesObject:)
    @NSManaged public func addToWorkspaceSlides(_ value: WorkspaceSlide)

    @objc(removeWorkspaceSlidesObject:)
    @NSManaged public func removeFromWorkspaceSlides(_ value: WorkspaceSlide)

    @objc(addWorkspaceSlides:)
    @NSManaged public func addToWorkspaceSlides(_ values: NSOrderedSet)

    @objc(removeWorkspaceSlides:)
    @NSManaged public func removeFromWorkspaceSlides(_ values: NSOrderedSet)
}

extension Workspace: Identifiable {
    public var id: UUID { workspaceId }
}
