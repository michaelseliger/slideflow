//
//  CoreDataStack.swift
//  Slideflow
//
//  Core Data persistence stack and error handling
//

import Foundation
import CoreData

enum CoreDataError: Error {
    case saveFailed(underlying: Error)
    case fetchFailed(underlying: Error)
    case validationFailed(message: String)
    case entityNotFound
    case duplicateEntry

    var localizedDescription: String {
        switch self {
        case .saveFailed(let error):
            return "Failed to save: \(error.localizedDescription)"
        case .fetchFailed(let error):
            return "Failed to fetch: \(error.localizedDescription)"
        case .validationFailed(let message):
            return "Validation failed: \(message)"
        case .entityNotFound:
            return "Entity not found"
        case .duplicateEntry:
            return "Duplicate entry"
        }
    }
}

class CoreDataStack {
    static let shared = CoreDataStack()

    let container: NSPersistentContainer

    var viewContext: NSManagedObjectContext {
        container.viewContext
    }

    private init() {
        container = NSPersistentContainer(name: "Slideflow")

        container.loadPersistentStores { description, error in
            if let error = error {
                fatalError("Core Data failed to load: \(error.localizedDescription)")
            }
        }

        container.viewContext.automaticallyMergesChangesFromParent = true
        container.viewContext.mergePolicy = NSMergePolicy.mergeByPropertyObjectTrump
    }

    func newBackgroundContext() -> NSManagedObjectContext {
        let context = container.newBackgroundContext()
        context.mergePolicy = NSMergePolicy.mergeByPropertyObjectTrump
        return context
    }

    func save(context: NSManagedObjectContext) -> Result<Void, CoreDataError> {
        guard context.hasChanges else {
            return .success(())
        }

        do {
            try context.save()
            return .success(())
        } catch {
            return .failure(.saveFailed(underlying: error))
        }
    }

    func performBackgroundTask<T>(_ block: @escaping (NSManagedObjectContext) -> Result<T, CoreDataError>) async -> Result<T, CoreDataError> {
        await withCheckedContinuation { continuation in
            container.performBackgroundTask { context in
                let result = block(context)
                continuation.resume(returning: result)
            }
        }
    }
}
