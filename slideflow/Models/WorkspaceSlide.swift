//
//  WorkspaceSlide.swift
//  Slideflow
//
//  NSManagedObject subclass for WorkspaceSlide entity
//

import Foundation
import CoreData

@objc(WorkspaceSlide)
public class WorkspaceSlide: NSManagedObject {
    @NSManaged public var workspaceSlideId: UUID
    @NSManaged public var orderIndex: Int16
    @NSManaged public var addedDate: Date
    @NSManaged public var notes: String?
    @NSManaged public var workspace: Workspace?
    @NSManaged public var slide: IndexedSlide?

    @nonobjc public class func fetchRequest() -> NSFetchRequest<WorkspaceSlide> {
        return NSFetchRequest<WorkspaceSlide>(entityName: "WorkspaceSlide")
    }
}

extension WorkspaceSlide: Identifiable {
    public var id: UUID { workspaceSlideId }
}
