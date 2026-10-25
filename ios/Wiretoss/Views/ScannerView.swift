import AVFoundation
import SwiftUI

struct ScannerSheet: View {
    @Environment(\.dismiss) private var dismiss
    let onCode: (String) -> Void

    @State private var errorMessage: String?

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()

            QRScannerView(
                onCode: { code in
                    onCode(code)
                    dismiss()
                },
                onError: { message in
                    errorMessage = message
                }
            )
            .ignoresSafeArea()

            ScanOverlay()
                .allowsHitTesting(false)

            VStack {
                HStack {
                    Spacer()
                    Button {
                        dismiss()
                    } label: {
                        Image(systemName: "xmark")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundStyle(.white)
                            .frame(width: 36, height: 36)
                            .background(.black.opacity(0.38), in: Circle())
                    }
                    .padding(.top, 10)
                    .padding(.trailing, 16)
                }
                Spacer()
            }

            if let errorMessage {
                VStack(spacing: 14) {
                    Image(systemName: "camera.fill")
                        .font(.system(size: 30))
                        .foregroundStyle(Color.wtDanger)
                    Text(errorMessage)
                        .font(.system(size: 14, weight: .medium))
                        .multilineTextAlignment(.center)
                        .foregroundStyle(.white)
                    Button("打开系统设置") {
                        if let url = URL(string: UIApplication.openSettingsURLString) {
                            UIApplication.shared.open(url)
                        }
                    }
                    .buttonStyle(WTSecondaryButtonStyle())
                }
                .padding(22)
                .frame(maxWidth: 320)
                .background {
                    RoundedRectangle(cornerRadius: 20, style: .continuous)
                        .fill(Color.wtSurface.opacity(0.96))
                }
                .padding(24)
            }
        }
        .preferredColorScheme(.dark)
    }
}

private struct ScanOverlay: View {
    var body: some View {
        GeometryReader { proxy in
            let side = min(proxy.size.width, proxy.size.height) * 0.68
            let frame = CGRect(
                x: (proxy.size.width - side) / 2,
                y: (proxy.size.height - side) / 2,
                width: side,
                height: side
            )

            Path { path in
                path.addRect(CGRect(origin: .zero, size: proxy.size))
                path.addRoundedRect(in: frame, cornerSize: CGSize(width: 30, height: 30))
            }
            .fill(Color.black.opacity(0.46), style: FillStyle(eoFill: true))

            RoundedRectangle(cornerRadius: 30, style: .continuous)
                .stroke(Color.wtAccent.opacity(0.92), lineWidth: 2)
                .frame(width: side, height: side)
                .position(x: proxy.size.width / 2, y: proxy.size.height / 2)

            VStack {
                Spacer()
                Text("把电脑上的二维码放进取景框")
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(.white)
                    .padding(.bottom, 50)
            }
            .frame(maxWidth: .infinity)
        }
        .ignoresSafeArea()
    }
}

struct QRScannerView: UIViewControllerRepresentable {
    let onCode: (String) -> Void
    let onError: (String) -> Void

    func makeUIViewController(context: Context) -> ScannerViewController {
        let controller = ScannerViewController()
        controller.onCode = onCode
        controller.onError = onError
        return controller
    }

    func updateUIViewController(_ uiViewController: ScannerViewController, context: Context) {}
}

final class ScannerViewController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var onCode: ((String) -> Void)?
    var onError: ((String) -> Void)?

    private let session = AVCaptureSession()
    private let sessionQueue = DispatchQueue(label: "dev.wiretoss.scanner")
    private var previewLayer: AVCaptureVideoPreviewLayer?
    private var hasReported = false
    private var isConfigured = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        requestCamera()
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        startSession()
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        sessionQueue.async { [session] in
            if session.isRunning { session.stopRunning() }
        }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        previewLayer?.frame = view.bounds
    }

    private func requestCamera() {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized:
            configure()
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .video) { [weak self] granted in
                DispatchQueue.main.async {
                    guard let self else { return }
                    if granted {
                        self.configure()
                    } else {
                        self.onError?("没有相机权限。请在系统设置里允许 Wiretoss 使用相机。")
                    }
                }
            }
        default:
            onError?("没有相机权限。请在系统设置里允许 Wiretoss 使用相机。")
        }
    }

    private func configure() {
        guard !isConfigured else { return }
        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input)
        else {
            onError?("无法打开相机。")
            return
        }

        session.beginConfiguration()
        session.addInput(input)

        let output = AVCaptureMetadataOutput()
        if session.canAddOutput(output) {
            session.addOutput(output)
            output.setMetadataObjectsDelegate(self, queue: .main)
            output.metadataObjectTypes = [.qr]
        }
        session.commitConfiguration()
        isConfigured = true

        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.videoGravity = .resizeAspectFill
        view.layer.addSublayer(preview)
        previewLayer = preview
        startSession()
    }

    private func startSession() {
        guard isConfigured, !session.isRunning else { return }
        sessionQueue.async { [session] in
            if !session.isRunning { session.startRunning() }
        }
    }

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard !hasReported,
              let object = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
              object.type == .qr,
              let value = object.stringValue
        else {
            return
        }
        hasReported = true
        session.stopRunning()
        onCode?(value)
    }
}
