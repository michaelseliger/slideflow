//
//  DirectoryMonitor.swift
//  Slideflow
//
//  FSEvents-based directory watching with debouncing
//

import Foundation

class DirectoryMonitor {
    private var eventStream: FSEventStreamRef?
    private var monitoredPath: String?
    private var debounceTimer: Timer?
    private var debounceInterval: TimeInterval = 0.5
    private var changeCallback: ((FSEventStreamEventFlags) -> Void)?

    init() {}

    deinit {
        stopMonitoring()
    }

    /// Start monitoring directory for changes
    func startMonitoring(path: String, debounceInterval: TimeInterval = 0.5, onChange: @escaping (FSEventStreamEventFlags) -> Void) {
        self.monitoredPath = path
        self.debounceInterval = debounceInterval
        self.changeCallback = onChange

        var context = FSEventStreamContext(
            version: 0,
            info: Unmanaged.passUnretained(self).toOpaque(),
            retain: nil,
            release: nil,
            copyDescription: nil
        )

        let callback: FSEventStreamCallback = { (
            streamRef,
            clientCallBackInfo,
            numEvents,
            eventPaths,
            eventFlags,
            eventIds
        ) in
            guard let info = clientCallBackInfo else { return }
            let monitor = Unmanaged<DirectoryMonitor>.fromOpaque(info).takeUnretainedValue()
            monitor.handleEvent(flags: eventFlags, numEvents: numEvents)
        }

        let pathsToWatch = [path] as CFArray
        let flags = UInt32(kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagFileEvents)

        eventStream = FSEventStreamCreate(
            nil,
            callback,
            &context,
            pathsToWatch,
            FSEventStreamEventId(kFSEventStreamEventIdSinceNow),
            0.3, // Latency
            flags
        )

        if let stream = eventStream {
            FSEventStreamScheduleWithRunLoop(stream, CFRunLoopGetCurrent(), CFRunLoopMode.defaultMode.rawValue)
            FSEventStreamStart(stream)
        }
    }

    /// Stop monitoring
    func stopMonitoring() {
        debounceTimer?.invalidate()
        debounceTimer = nil

        if let stream = eventStream {
            FSEventStreamStop(stream)
            FSEventStreamInvalidate(stream)
            FSEventStreamRelease(stream)
            eventStream = nil
        }
    }

    /// Handle file system event with debouncing
    private func handleEvent(flags: UnsafePointer<FSEventStreamEventFlags>, numEvents: Int) {
        // Invalidate existing timer
        debounceTimer?.invalidate()

        // Schedule debounced callback
        debounceTimer = Timer.scheduledTimer(withTimeInterval: debounceInterval, repeats: false) { [weak self] _ in
            guard let self = self else { return }

            // Call the change callback with aggregated flags
            if numEvents > 0 {
                let eventFlags = flags[0]
                self.changeCallback?(eventFlags)
            }
        }
    }

    /// Check if a path should trigger re-indexing
    static func shouldReindex(eventFlags: FSEventStreamEventFlags) -> Bool {
        let created = (eventFlags & FSEventStreamEventFlags(kFSEventStreamEventFlagItemCreated)) != 0
        let modified = (eventFlags & FSEventStreamEventFlags(kFSEventStreamEventFlagItemModified)) != 0
        let removed = (eventFlags & FSEventStreamEventFlags(kFSEventStreamEventFlagItemRemoved)) != 0

        return created || modified || removed
    }
}
