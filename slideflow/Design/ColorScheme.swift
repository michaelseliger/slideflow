//
//  ColorScheme.swift
//  Slideflow
//
//  Modern color palette for marketers
//

import SwiftUI

extension Color {
    // MARK: - Primary Brand Colors

    /// Main brand color - vibrant purple
    static let brandPrimary = Color(red: 0.46, green: 0.29, blue: 0.89) // #7549E3

    /// Secondary brand color - soft coral
    static let brandSecondary = Color(red: 1.0, green: 0.45, blue: 0.45) // #FF7373

    /// Accent color - warm orange
    static let brandAccent = Color(red: 1.0, green: 0.62, blue: 0.29) // #FF9E4A

    /// Success color - fresh green
    static let brandSuccess = Color(red: 0.24, green: 0.82, blue: 0.55) // #3DD18C

    // MARK: - Neutral Colors

    /// Background - soft off-white
    static let backgroundPrimary = Color(red: 0.98, green: 0.98, blue: 0.99) // #FAFAFC

    /// Card background - pure white
    static let backgroundCard = Color.white

    /// Subtle background - light gray
    static let backgroundSubtle = Color(red: 0.95, green: 0.96, blue: 0.98) // #F3F4FA

    /// Border color - light purple tint
    static let borderLight = Color(red: 0.91, green: 0.91, blue: 0.95) // #E8E8F2

    // MARK: - Text Colors

    /// Primary text - dark charcoal
    static let textPrimary = Color(red: 0.15, green: 0.16, blue: 0.21) // #262935

    /// Secondary text - medium gray
    static let textSecondary = Color(red: 0.51, green: 0.54, blue: 0.62) // #828A9E

    /// Tertiary text - light gray
    static let textTertiary = Color(red: 0.69, green: 0.72, blue: 0.79) // #B0B7C9
}

// MARK: - Gradient Helpers

extension LinearGradient {
    /// Purple to pink gradient
    static let brandGradient = LinearGradient(
        colors: [Color.brandPrimary, Color.brandSecondary],
        startPoint: .topLeading,
        endPoint: .bottomTrailing
    )

    /// Warm sunset gradient
    static let accentGradient = LinearGradient(
        colors: [Color.brandAccent, Color.brandSecondary],
        startPoint: .leading,
        endPoint: .trailing
    )
}
