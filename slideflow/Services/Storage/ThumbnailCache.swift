//
//  ThumbnailCache.swift
//  Slideflow
//
//  LRU in-memory cache and orphan cleanup for slide thumbnails
//

import Foundation
import AppKit

class ThumbnailCache {
    private let maxCacheSize: Int
    private var cache: NSCache<NSString, NSImage>
    private let fileManager = FileManager.default

    private var cacheDirectory: URL {
        fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Slideflow/Thumbnails")
    }

    init(maxCacheSize: Int = 100) {
        self.maxCacheSize = maxCacheSize
        self.cache = NSCache<NSString, NSImage>()
        self.cache.countLimit = maxCacheSize

        // Ensure cache directory exists
        try? fileManager.createDirectory(at: cacheDirectory, withIntermediateDirectories: true)
    }

    /// Get thumbnail from cache or disk
    func getThumbnail(for path: String) -> NSImage? {
        let key = path as NSString

        // Check memory cache first
        if let cached = cache.object(forKey: key) {
            return cached
        }

        // Load from disk
        guard let image = NSImage(contentsOfFile: path) else {
            return nil
        }

        // Store in memory cache
        cache.setObject(image, forKey: key)
        return image
    }

    /// Store thumbnail in cache
    func storeThumbnail(_ image: NSImage, for path: String) {
        let key = path as NSString
        cache.setObject(image, forKey: key)
    }

    /// Clear memory cache
    func clearMemoryCache() {
        cache.removeAllObjects()
    }

    /// Remove orphaned thumbnails (no corresponding IndexedSlide)
    func cleanupOrphanedThumbnails(validPaths: Set<String>) -> Result<Int, Error> {
        do {
            let files = try fileManager.contentsOfDirectory(at: cacheDirectory, includingPropertiesForKeys: nil)
            var removedCount = 0

            for file in files {
                if !validPaths.contains(file.path) {
                    try fileManager.removeItem(at: file)
                    removedCount += 1
                }
            }

            return .success(removedCount)
        } catch {
            return .failure(error)
        }
    }

    /// Get cache size in bytes
    func getCacheSize() -> UInt64 {
        guard let files = try? fileManager.contentsOfDirectory(at: cacheDirectory, includingPropertiesForKeys: [.fileSizeKey]) else {
            return 0
        }

        return files.reduce(0) { total, file in
            guard let size = try? file.resourceValues(forKeys: [.fileSizeKey]).fileSize else {
                return total
            }
            return total + UInt64(size)
        }
    }
}
