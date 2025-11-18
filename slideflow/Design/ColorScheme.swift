//
//  ColorScheme.swift
//  Slideflow
//
//  Adaptive color system for light and dark mode
//

import SwiftUI
import AppKit

extension Color {
    // MARK: - Adaptive Brand Colors

    /// Main brand color - vibrant purple (adapts to light/dark mode)
    static var brandPrimary: Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            if appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua {
                // Lighter, more vibrant purple for dark mode
                return NSColor(red: 0.60, green: 0.40, blue: 0.95, alpha: 1.0)
            } else {
                // Original purple for light mode
                return NSColor(red: 0.46, green: 0.29, blue: 0.89, alpha: 1.0)
            }
        })
    }

    /// Secondary brand color - soft coral (adapts to light/dark mode)
    static var brandSecondary: Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            if appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua {
                // Softer coral for dark mode
                return NSColor(red: 1.0, green: 0.55, blue: 0.55, alpha: 1.0)
            } else {
                // Original coral for light mode
                return NSColor(red: 1.0, green: 0.45, blue: 0.45, alpha: 1.0)
            }
        })
    }

    /// Accent color - warm orange (adapts to light/dark mode)
    static var brandAccent: Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            if appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua {
                // Brighter orange for dark mode
                return NSColor(red: 1.0, green: 0.68, blue: 0.40, alpha: 1.0)
            } else {
                // Original orange for light mode
                return NSColor(red: 1.0, green: 0.62, blue: 0.29, alpha: 1.0)
            }
        })
    }

    /// Success color - fresh green (adapts to light/dark mode)
    static var brandSuccess: Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            if appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua {
                // Brighter green for dark mode
                return NSColor(red: 0.30, green: 0.88, blue: 0.60, alpha: 1.0)
            } else {
                // Original green for light mode
                return NSColor(red: 0.24, green: 0.82, blue: 0.55, alpha: 1.0)
            }
        })
    }

    // MARK: - Semantic System Colors (Auto-adapt)

    /// Primary background - window background (auto-adapts)
    static var backgroundPrimary: Color {
        Color(nsColor: .windowBackgroundColor)
    }

    /// Card/Control background (auto-adapts)
    static var backgroundCard: Color {
        Color(nsColor: .controlBackgroundColor)
    }

    /// Subtle background for secondary elements (auto-adapts)
    static var backgroundSubtle: Color {
        Color(nsColor: .quaternaryLabelColor).opacity(0.3)
    }

    /// Under page background (auto-adapts)
    static var backgroundUnderPage: Color {
        Color(nsColor: .underPageBackgroundColor)
    }

    /// Border/separator color (auto-adapts)
    static var borderLight: Color {
        Color(nsColor: .separatorColor)
    }

    // MARK: - Semantic Text Colors (Auto-adapt)

    /// Primary text color (auto-adapts)
    static var textPrimary: Color {
        Color(nsColor: .labelColor)
    }

    /// Secondary text color (auto-adapts)
    static var textSecondary: Color {
        Color(nsColor: .secondaryLabelColor)
    }

    /// Tertiary text color (auto-adapts)
    static var textTertiary: Color {
        Color(nsColor: .tertiaryLabelColor)
    }

    /// Quaternary text color for de-emphasized content (auto-adapts)
    static var textQuaternary: Color {
        Color(nsColor: .quaternaryLabelColor)
    }
}

// MARK: - Gradient Helpers

extension LinearGradient {
    /// Purple to pink gradient (adapts to mode via brand colors)
    static var brandGradient: LinearGradient {
        LinearGradient(
            colors: [Color.brandPrimary, Color.brandSecondary],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )
    }

    /// Warm sunset gradient (adapts to mode via brand colors)
    static var accentGradient: LinearGradient {
        LinearGradient(
            colors: [Color.brandAccent, Color.brandSecondary],
            startPoint: .leading,
            endPoint: .trailing
        )
    }
}
