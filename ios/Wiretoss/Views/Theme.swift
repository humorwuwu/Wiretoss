import SwiftUI

extension Color {
    static let wtBackground = Color(red: 11 / 255, green: 16 / 255, blue: 32 / 255)
    static let wtBackgroundDeep = Color(red: 7 / 255, green: 10 / 255, blue: 21 / 255)
    static let wtSurface = Color(red: 21 / 255, green: 27 / 255, blue: 46 / 255)
    static let wtText = Color(red: 232 / 255, green: 236 / 255, blue: 245 / 255)
    static let wtTextDim = Color(red: 138 / 255, green: 147 / 255, blue: 168 / 255)
    static let wtTextFaint = Color(red: 94 / 255, green: 103 / 255, blue: 128 / 255)
    static let wtAccent = Color(red: 91 / 255, green: 140 / 255, blue: 255 / 255)
    static let wtAccent2 = Color(red: 139 / 255, green: 92 / 255, blue: 246 / 255)
    static let wtDanger = Color(red: 251 / 255, green: 113 / 255, blue: 133 / 255)
    static let wtOk = Color(red: 52 / 255, green: 211 / 255, blue: 153 / 255)
}

struct WTGradientBackground: View {
    var body: some View {
        ZStack {
            LinearGradient(
                colors: [Color.wtBackground, Color.wtBackgroundDeep],
                startPoint: .top,
                endPoint: .bottom
            )
            .ignoresSafeArea()

            Circle()
                .fill(Color.wtAccent.opacity(0.22))
                .frame(width: 460, height: 460)
                .blur(radius: 110)
                .offset(x: -190, y: -360)

            Circle()
                .fill(Color.wtAccent2.opacity(0.18))
                .frame(width: 420, height: 420)
                .blur(radius: 120)
                .offset(x: 210, y: 300)
        }
        .ignoresSafeArea()
    }
}

private struct WTCardModifier: ViewModifier {
    let padding: CGFloat

    func body(content: Content) -> some View {
        content
            .padding(padding)
            .background {
                RoundedRectangle(cornerRadius: 22, style: .continuous)
                    .fill(.ultraThinMaterial)
                    .overlay {
                        RoundedRectangle(cornerRadius: 22, style: .continuous)
                            .fill(Color.wtSurface.opacity(0.52))
                    }
                    .overlay {
                        RoundedRectangle(cornerRadius: 22, style: .continuous)
                            .stroke(Color.white.opacity(0.08), lineWidth: 1)
                    }
            }
            .shadow(color: .black.opacity(0.28), radius: 24, x: 0, y: 14)
    }
}

extension View {
    func wtCard(padding: CGFloat = 20) -> some View {
        modifier(WTCardModifier(padding: padding))
    }
}

struct WTPrimaryButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 15, weight: .semibold))
            .foregroundStyle(.white)
            .frame(maxWidth: .infinity, minHeight: 48)
            .background {
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                    .fill(
                        LinearGradient(
                            colors: [.wtAccent, .wtAccent2],
                            startPoint: .topLeading,
                            endPoint: .bottomTrailing
                        )
                    )
            }
            .shadow(color: .wtAccent.opacity(0.28), radius: 14, x: 0, y: 8)
            .opacity(configuration.isPressed ? 0.82 : 1)
            .scaleEffect(configuration.isPressed ? 0.99 : 1)
            .animation(.easeOut(duration: 0.15), value: configuration.isPressed)
    }
}

struct WTSecondaryButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 14, weight: .medium))
            .foregroundStyle(Color.wtText)
            .frame(maxWidth: .infinity, minHeight: 46)
            .background {
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                    .fill(Color.white.opacity(configuration.isPressed ? 0.10 : 0.055))
                    .overlay {
                        RoundedRectangle(cornerRadius: 13, style: .continuous)
                            .stroke(Color.white.opacity(0.10), lineWidth: 1)
                    }
            }
            .animation(.easeOut(duration: 0.15), value: configuration.isPressed)
    }
}

struct WTLogo: View {
    var size: CGFloat = 42

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
                .fill(
                    LinearGradient(
                        colors: [
                            Color(red: 0.435, green: 0.616, blue: 1.0),
                            .wtAccent,
                            .wtAccent2,
                        ],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    )
                )

            Image(systemName: "arrow.right")
                .font(.system(size: size * 0.33, weight: .bold))
                .foregroundStyle(.white)
                .offset(x: size * 0.03, y: -size * 0.12)

            Image(systemName: "arrow.left")
                .font(.system(size: size * 0.33, weight: .bold))
                .foregroundStyle(.white.opacity(0.72))
                .offset(x: -size * 0.03, y: size * 0.12)
        }
        .frame(width: size, height: size)
        .shadow(color: .wtAccent.opacity(0.36), radius: size * 0.22, x: 0, y: size * 0.1)
    }
}

/// 一行地址输入框的通用外观。
struct WTAddressField: View {
    let placeholder: String
    @Binding var text: String
    var focused: FocusState<Bool>.Binding

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "globe")
                .font(.system(size: 16, weight: .medium))
                .foregroundStyle(Color.wtTextFaint)

            TextField(placeholder, text: $text)
                .font(.system(size: 15, design: .monospaced))
                .foregroundStyle(Color.wtText)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(.URL)
                .textContentType(.URL)
                .focused(focused)
                .submitLabel(.go)
        }
        .padding(.horizontal, 14)
        .frame(height: 48)
        .background {
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .fill(Color.black.opacity(0.34))
                .overlay {
                    RoundedRectangle(cornerRadius: 13, style: .continuous)
                        .stroke(
                            focused.wrappedValue ? Color.wtAccent.opacity(0.65) : Color.white.opacity(0.09),
                            lineWidth: 1
                        )
                }
        }
    }
}
