//
//  SearchFilter.swift
//  Slideflow
//
//  Filter logic for presentation name, file path, date range
//

import Foundation
import CoreData

protocol Filter {
    func predicate() -> NSPredicate
}

struct PresentationNameFilter: Filter {
    let name: String

    func predicate() -> NSPredicate {
        return NSPredicate(format: "sourcePresentation.filename CONTAINS[cd] %@", name)
    }
}

struct FileLocationFilter: Filter {
    let directoryPath: String

    func predicate() -> NSPredicate {
        return NSPredicate(format: "sourcePresentation.directory.path == %@", directoryPath)
    }
}

struct DateRangeFilter: Filter {
    let start: Date
    let end: Date

    func predicate() -> NSPredicate {
        return NSPredicate(
            format: "sourcePresentation.fileModifiedDate >= %@ AND sourcePresentation.fileModifiedDate <= %@",
            start as NSDate,
            end as NSDate
        )
    }
}

class SearchFilter {
    init() {}

    /// Apply single filter
    func applyFilter(_ filter: Filter, in context: NSManagedObjectContext) async -> Result<[IndexedSlide], CoreDataError> {
        return await withCheckedContinuation { continuation in
            context.perform {
                let fetchRequest: NSFetchRequest<IndexedSlide> = IndexedSlide.fetchRequest()
                fetchRequest.predicate = filter.predicate()
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

    /// Apply multiple filters (AND logic)
    func applyCombinedFilters(_ filters: [Filter], in context: NSManagedObjectContext) async -> Result<[IndexedSlide], CoreDataError> {
        return await withCheckedContinuation { continuation in
            context.perform {
                let predicates = filters.map { $0.predicate() }
                let compoundPredicate = NSCompoundPredicate(andPredicateWithSubpredicates: predicates)

                let fetchRequest: NSFetchRequest<IndexedSlide> = IndexedSlide.fetchRequest()
                fetchRequest.predicate = compoundPredicate
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

    /// Search with filters
    func searchWithFilters(query: String, filters: [Filter], in context: NSManagedObjectContext) async -> Result<[IndexedSlide], CoreDataError> {
        return await withCheckedContinuation { continuation in
            context.perform {
                var predicates: [NSPredicate] = filters.map { $0.predicate() }

                // Add query predicate if not empty
                if !query.isEmpty {
                    predicates.append(NSPredicate(format: "textContent CONTAINS[cd] %@", query))
                }

                let compoundPredicate = NSCompoundPredicate(andPredicateWithSubpredicates: predicates)

                let fetchRequest: NSFetchRequest<IndexedSlide> = IndexedSlide.fetchRequest()
                fetchRequest.predicate = compoundPredicate
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
}
