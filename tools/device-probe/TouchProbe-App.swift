// The device panel's input probe app (tests and `flare-sim-host --input-bench` only, never shipped; moved in from
// spike S1, research/flare/2026-11-19-ios-simulator-input/TouchProbe).
//
import SwiftUI
import UIKit

// S1 input probe (2026-11-19). Every displayed frame paints, as 16-bit barcodes (rows of 16 black/white
// blocks, 24 pt tall, from y = 120 pt), the app frame number and the state of the last touch and key it saw,
// so a host reading the framebuffer can time press-to-painted-frame exactly:
//   row 0 frame number            row 1 touchesBegan count       row 2 last began x * 50
//   row 3 last began y * 50       row 4 frame number at last began row 5 touchesEnded count
//   row 6 characters typed        row 7 most touches down at once  row 8 touchesMoved count
// Every event is also appended to Documents/log.txt with full precision.
final class ProbeView: UIView, UIKeyInput {
    private var link: CADisplayLink?
    private var rows = [UInt16](repeating: 0, count: 9)
    private var text = ""
    private var log: FileHandle?

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .darkGray
        isMultipleTouchEnabled = true
        let url = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("log.txt")
        FileManager.default.createFile(atPath: url.path, contents: nil)
        log = try? FileHandle(forWritingTo: url)
        link = CADisplayLink(target: self, selector: #selector(tick))
        link?.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 60, preferred: 60)
        link?.add(to: .main, forMode: .common)
    }
    required init?(coder: NSCoder) { fatalError() }

    private func write(_ s: String) { log?.write((s + "\n").data(using: .utf8)!) }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil {
            write("S \(bounds.width) \(bounds.height) \(window!.screen.scale)")
            DispatchQueue.main.async { _ = self.becomeFirstResponder() }
        }
    }
    override func layoutSubviews() { super.layoutSubviews(); write("L \(bounds.width) \(bounds.height)") }

    @objc private func tick() { rows[0] &+= 1; setNeedsDisplay() }

    // Touches
    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        let all = event?.allTouches?.count ?? touches.count
        rows[7] = max(rows[7], UInt16(all))
        for t in touches {
            let p = t.location(in: self)
            rows[1] &+= 1
            rows[2] = UInt16(clamping: Int((p.x * 50).rounded()))
            rows[3] = UInt16(clamping: Int((p.y * 50).rounded()))
            rows[4] = rows[0]
            write("B \(rows[1]) \(rows[0]) \(p.x) \(p.y) all=\(all) t=\(t.timestamp)")
        }
        _ = becomeFirstResponder()
    }
    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        let all = event?.allTouches?.count ?? touches.count
        rows[7] = max(rows[7], UInt16(all))
        for t in touches {
            let p = t.location(in: self)
            rows[8] &+= 1
            write("M \(rows[8]) \(rows[0]) \(p.x) \(p.y) all=\(all)")
        }
    }
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        for t in touches {
            let p = t.location(in: self)
            rows[5] &+= 1
            write("E \(rows[5]) \(rows[0]) \(p.x) \(p.y)")
        }
    }
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        for t in touches { let p = t.location(in: self); write("C \(rows[0]) \(p.x) \(p.y)") }
    }

    // Keys (hardware keyboard events; an empty inputView keeps the software keyboard away)
    override var canBecomeFirstResponder: Bool { true }
    override var inputView: UIView? { UIView(frame: .zero) }
    // Run 1 showed iOS smart quotes turning ' and " into curly ones; turn text substitutions off so the
    // probe compares raw characters.
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var smartInsertDeleteType: UITextSmartInsertDeleteType = .no
    var autocorrectionType: UITextAutocorrectionType = .no
    var autocapitalizationType: UITextAutocapitalizationType = .none
    var spellCheckingType: UITextSpellCheckingType = .no
    var hasText: Bool { !text.isEmpty }
    func insertText(_ s: String) {
        text += s
        rows[6] = UInt16(clamping: text.count)
        write("K \(text.count) \(s.unicodeScalars.map { String($0.value) }.joined(separator: ","))")
    }
    func deleteBackward() { if !text.isEmpty { text.removeLast() }; rows[6] = UInt16(clamping: text.count); write("D \(text.count)") }

    override func draw(_ rect: CGRect) {
        guard let c = UIGraphicsGetCurrentContext() else { return }
        // The band is as wide as the short side and centred, so in landscape it stays clear of the Dynamic
        // Island at either end (the input bench reads it there too).
        let w = min(bounds.width, bounds.height) / 16
        let x0 = (bounds.width - 16 * w) / 2
        for (r, v) in rows.enumerated() {
            for i in 0..<16 {
                let on = (v >> UInt16(15 - i)) & 1 == 1
                c.setFillColor(on ? UIColor.white.cgColor : UIColor.black.cgColor)
                c.fill(CGRect(x: x0 + CGFloat(i) * w, y: 120 + CGFloat(r) * 24, width: w, height: 24))
            }
        }
    }
}

struct ProbeRep: UIViewRepresentable {
    func makeUIView(context: Context) -> ProbeView { ProbeView() }
    func updateUIView(_ v: ProbeView, context: Context) {}
}

@main struct TouchProbeApp: App {
    var body: some Scene { WindowGroup { ProbeRep().ignoresSafeArea() } }
}
