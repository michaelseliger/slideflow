//
//  SlideflowApp.swift
//  Slideflow
//
//  Created by Michael Seliger on 18.11.25.
//

import SwiftUI

@main
struct SlideflowApp: App {
    let coreDataStack = CoreDataStack.shared

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environment(\.managedObjectContext, coreDataStack.viewContext)
        }
    }
}
