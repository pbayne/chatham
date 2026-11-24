// mic-monitor — emits one JSON line per mic start/stop, naming the app that
// opened the microphone. Chatham's ambient meeting detection (mic_detect.rs)
// consumes these to offer recording a call that was never on the calendar.
//
// Output (stdout, line-unbuffered), matching the Rust MicEvent contract:
//   {"event":"start","app_id":"us.zoom.xos","app_name":"zoom.us","pid":1234,"ts":1.5}
//   {"event":"stop","app_id":"us.zoom.xos","app_name":"zoom.us","pid":1234,"ts":2.0}
//
// macOS 14+ attributes each event to a process via the audio process-object
// API. On 13.x we fall back to a device-level signal and emit app-less events
// (the consumer's device-level gate decides whether to act — see mic_detect.rs).
//
// If the parent (the Tauri app) dies, the next stdout write gets SIGPIPE and we
// exit; the parent owns the respawn. Ported from the pattern in stenoai's
// mic_monitor.swift, adapted to Chatham's sidecar conventions.

import Foundation
import CoreAudio
import AppKit

setbuf(stdout, nil)

struct MicEvent: Codable {
    let event: String
    let app_id: String?
    let app_name: String?
    let pid: Int32?
    let ts: Double
}

let encoder = JSONEncoder()

func emit(_ event: MicEvent) {
    if let data = try? encoder.encode(event), let line = String(data: data, encoding: .utf8) {
        print(line)
    }
}

func defaultInputDevice() -> AudioDeviceID? {
    var deviceID: AudioDeviceID = 0
    var size = UInt32(MemoryLayout<AudioDeviceID>.size)
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyDefaultInputDevice,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    let status = AudioObjectGetPropertyData(
        AudioObjectID(kAudioObjectSystemObject),
        &address, 0, nil, &size, &deviceID
    )
    return (status == noErr && deviceID != 0) ? deviceID : nil
}

func deviceIsRunningSomewhere(_ deviceID: AudioDeviceID) -> Bool {
    var isRunning: UInt32 = 0
    var size = UInt32(MemoryLayout<UInt32>.size)
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioDevicePropertyDeviceIsRunningSomewhere,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    return AudioObjectGetPropertyData(deviceID, &address, 0, nil, &size, &isRunning) == noErr
        && isRunning != 0
}

@available(macOS 14.0, *)
func processObjectsCapturingInput() -> [pid_t: AudioObjectID] {
    var listAddr = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyProcessObjectList,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(
        AudioObjectID(kAudioObjectSystemObject), &listAddr, 0, nil, &size
    ) == noErr else { return [:] }

    let count = Int(size) / MemoryLayout<AudioObjectID>.size
    if count == 0 { return [:] }
    var processes = [AudioObjectID](repeating: 0, count: count)
    guard AudioObjectGetPropertyData(
        AudioObjectID(kAudioObjectSystemObject), &listAddr, 0, nil, &size, &processes
    ) == noErr else { return [:] }

    var inputAddr = AudioObjectPropertyAddress(
        mSelector: kAudioProcessPropertyIsRunningInput,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var pidAddr = AudioObjectPropertyAddress(
        mSelector: kAudioProcessPropertyPID,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )

    var procByPID: [pid_t: AudioObjectID] = [:]
    for proc in processes {
        var isRunningInput: UInt32 = 0
        var s1 = UInt32(MemoryLayout<UInt32>.size)
        guard AudioObjectGetPropertyData(proc, &inputAddr, 0, nil, &s1, &isRunningInput) == noErr,
              isRunningInput != 0
        else { continue }

        var pid: pid_t = 0
        var s2 = UInt32(MemoryLayout<pid_t>.size)
        if AudioObjectGetPropertyData(proc, &pidAddr, 0, nil, &s2, &pid) == noErr {
            procByPID[pid] = proc
        }
    }
    return procByPID
}

// coreaudiod records the client bundle id for every process object it vends —
// more reliable than NSRunningApplication for the helper/XPC children that
// browsers and meeting apps actually open the mic from.
@available(macOS 14.0, *)
func processBundleID(_ proc: AudioObjectID) -> String? {
    var addr = AudioObjectPropertyAddress(
        mSelector: kAudioProcessPropertyBundleID,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var size = UInt32(MemoryLayout<CFString?>.size)
    var value: CFString?
    let status = withUnsafeMutablePointer(to: &value) {
        AudioObjectGetPropertyData(proc, &addr, 0, nil, &size, $0)
    }
    guard status == noErr, let s = value as String?, !s.isEmpty else { return nil }
    return s
}

func processPath(_ pid: pid_t) -> String? {
    var buf = [CChar](repeating: 0, count: 4096)
    let n = proc_pidpath(pid, &buf, UInt32(buf.count))
    guard n > 0 else { return nil }
    return String(cString: buf)
}

// Helpers live inside the owning .app bundle, so the exe path usually contains
// it (…/Foo.app/Contents/…). Last resort when CoreAudio has no bundle id.
func bundleIDFromPath(_ path: String) -> String? {
    guard let range = path.range(of: ".app/") else { return nil }
    let appPath = String(path[..<range.lowerBound]) + ".app"
    return Bundle(path: appPath)?.bundleIdentifier
}

@available(macOS 14.0, *)
func appInfo(for pid: pid_t, procObject: AudioObjectID?) -> (bundleId: String?, name: String?) {
    if let app = NSRunningApplication(processIdentifier: pid), let bid = app.bundleIdentifier {
        return (bid, app.localizedName)
    }
    var bundleId: String? = procObject.flatMap { processBundleID($0) }
    var name: String? = nil
    if let path = processPath(pid) {
        name = (path as NSString).lastPathComponent
        if bundleId == nil { bundleId = bundleIDFromPath(path) }
    }
    return (bundleId, name)
}

let POLL_INTERVAL: TimeInterval = 1.0
// Sentinel for the macOS 13 fallback: device is in use, no per-process attribution.
let FALLBACK_PID: pid_t = -1

var lastActivePIDs: Set<pid_t> = []
// Info resolved at start, replayed at stop — the process (and its CoreAudio
// object) is usually gone by the time the mic is released.
var infoCache: [pid_t: (String?, String?)] = [:]

while true {
    var procByPID: [pid_t: AudioObjectID] = [:]
    var currentPIDs: Set<pid_t> = []

    if #available(macOS 14.0, *) {
        procByPID = processObjectsCapturingInput()
        currentPIDs = Set(procByPID.keys)
    } else if let device = defaultInputDevice(), deviceIsRunningSomewhere(device) {
        currentPIDs = [FALLBACK_PID]
    }

    let now = Date().timeIntervalSince1970

    for pid in currentPIDs.subtracting(lastActivePIDs) {
        var info: (String?, String?) = (nil, nil)
        if pid != FALLBACK_PID, #available(macOS 14.0, *) {
            info = appInfo(for: pid, procObject: procByPID[pid])
        }
        infoCache[pid] = info
        emit(MicEvent(
            event: "start",
            app_id: info.0,
            app_name: info.1,
            pid: pid == FALLBACK_PID ? nil : pid,
            ts: now
        ))
    }
    for pid in lastActivePIDs.subtracting(currentPIDs) {
        let info = infoCache.removeValue(forKey: pid) ?? (nil, nil)
        emit(MicEvent(
            event: "stop",
            app_id: info.0,
            app_name: info.1,
            pid: pid == FALLBACK_PID ? nil : pid,
            ts: now
        ))
    }

    lastActivePIDs = currentPIDs
    Thread.sleep(forTimeInterval: POLL_INTERVAL)
}
