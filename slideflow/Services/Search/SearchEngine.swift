//
//  SearchEngine.swift
//  Slideflow
//
//  CoreData NSPredicate-based full-text search with <2s performance
//

import Foundation
import CoreData

class SearchEngine {
    init() {}

    /// Perform full-text search on slide content
    func search(query: String, in context: NSManagedObjectContext) async -> Result<[IndexedSlide], CoreDataError> {
        return await withCheckedContinuation { continuation in
            context.perform {
                let fetchRequest: NSFetchRequest<IndexedSlide> = IndexedSlide.fetchRequest()

                // Build predicate
                if query.isEmpty {
                    // Return all slides
                    fetchRequest.predicate = nil
                } else {
                    // Case-insensitive CONTAINS search
                    fetchRequest.predicate = NSPredicate(format: "textContent CONTAINS[cd] %@", query)
                }

                // Sort by search rank (can be improved with relevance scoring)
                fetchRequest.sortDescriptors = [
                    NSSortDescriptor(key: "searchRank", ascending: false),
                    NSSortDescriptor(key: "extractedDate", ascending: false)
                ]

                do {
                    let results = try context.fetch(fetchRequest)
                    continuation.resume(returning: .success(results))
                } catch {
                    continuation.resume(returning: .failure(.fetchFailed(underlying: error)))
                }
            }
        }
    }

    /// Search with pagination
    func search(query: String, limit: Int, offset: Int, in context: NSManagedObjectContext) async -> Result<[IndexedSlide], CoreDataError> {
        return await withCheckedContinuation { continuation in
            context.perform {
                let fetchRequest: NSFetchRequest<IndexedSlide> = IndexedSlide.fetchRequest()
                fetchRequest.predicate = query.isEmpty ? nil : NSPredicate(format: "textContent CONTAINS[cd] %@", query)
                fetchRequest.fetchLimit = limit
                fetchRequest.fetchOffset = offset
                fetchRequest.sortDescriptors = [NSSortDescriptor(key: "extractedDate", ascending: false)]

                do {
                    let results = try context.fetch(fetchRequest)
                    continuation.resume(returning: .success(results))
                } catch {
                    continuation.resume(returning: .failure(.fetchFailed(underlying: error)))
                }
            }
        }
    }

    /// Count total results for query
    func count(query: String, in context: NSManagedObjectContext) async -> Result<Int, CoreDataError> {
        return await withCheckedContinuation { continuation in
            context.perform {
                let fetchRequest: NSFetchRequest<IndexedSlide> = IndexedSlide.fetchRequest()
                fetchRequest.predicate = query.isEmpty ? nil : NSPredicate(format: "textContent CONTAINS[cd] %@", query)

                do {
                    let count = try context.count(for: fetchRequest)
                    continuation.resume(returning: .success(count))
                } catch {
                    continuation.resume(returning: .failure(.fetchFailed(underlying: error)))
                }
            }
        }
    }
}
