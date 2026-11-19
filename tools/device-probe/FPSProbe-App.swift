// The device panel's probe app (tests and the bench only, never shipped; moved in from the 2026-11-17 research).

import SwiftUI
import UIKit

// Every displayed frame paints its frame index as a 16-bit barcode (black/white blocks)
// in the top band, so a capture tool can count distinct frames and gaps exactly.
final class ProbeView: UIView {
    private var link: CADisplayLink?
    private var frame16: UInt16 = 0
    private var bar: CGFloat = 0
    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .darkGray
        link = CADisplayLink(target: self, selector: #selector(tick))
        link?.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 60, preferred: 60)
        link?.add(to: .main, forMode: .common)
    }
    required init?(coder: NSCoder) { fatalError() }
    @objc private func tick() {
        frame16 &+= 1
        bar = (bar + 6).truncatingRemainder(dividingBy: bounds.width)
        setNeedsDisplay()
    }
    override func draw(_ rect: CGRect) {
        guard let c = UIGraphicsGetCurrentContext() else { return }
        let w = bounds.width / 16
        for i in 0..<16 {
            let on = (frame16 >> UInt16(15 - i)) & 1 == 1
            c.setFillColor(on ? UIColor.white.cgColor : UIColor.black.cgColor)
            c.fill(CGRect(x: CGFloat(i) * w, y: 120, width: w, height: 60))
        }
        if ProcessInfo.processInfo.arguments.contains("-heavy") {
            // Whole screen changes every frame: a moving hue gradient plus 300 random squares (worst case for the encoder).
            let t = CGFloat(frame16 % 360) / 360
            let colors = [UIColor(hue: t, saturation: 1, brightness: 1, alpha: 1).cgColor,
                          UIColor(hue: (t + 0.5).truncatingRemainder(dividingBy: 1), saturation: 1, brightness: 0.6, alpha: 1).cgColor] as CFArray
            let g = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: [0, 1])!
            c.saveGState(); c.clip(to: CGRect(x: 0, y: 200, width: bounds.width, height: bounds.height - 200))
            c.drawLinearGradient(g, start: CGPoint(x: 0, y: 200), end: CGPoint(x: bounds.width, y: bounds.height), options: [])
            c.restoreGState()
            var rng = SystemRandomNumberGenerator()
            for _ in 0..<300 {
                c.setFillColor(UIColor(white: CGFloat.random(in: 0...1, using: &rng), alpha: 1).cgColor)
                c.fill(CGRect(x: CGFloat.random(in: 0..<bounds.width, using: &rng), y: CGFloat.random(in: 200..<bounds.height, using: &rng), width: 24, height: 24))
            }
        }
        c.setFillColor(UIColor.systemOrange.cgColor)
        c.fill(CGRect(x: bar, y: 260, width: 40, height: bounds.height - 360))
    }
}

struct ProbeRep: UIViewRepresentable {
    func makeUIView(context: Context) -> ProbeView { ProbeView() }
    func updateUIView(_ v: ProbeView, context: Context) {}
}

@main struct FPSProbeApp: App {
    var body: some Scene { WindowGroup { ProbeRep().ignoresSafeArea() } }
}
