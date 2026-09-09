// TearHero — 20-20-20 eye-break reminder, as an Omarchy shell plugin.
//
// The binary (`tearhero`) is the single source of truth for phase, mode and
// audio. This widget only draws what `tearhero watch` streams and sends
// commands back. One instance of this widget (and so one watcher) exists per
// monitor; that is why no sound is ever played from QML — it would play once
// per screen — and why the binary claims each reminder chime with a lock file.
//
// Liveness matters: tearhero treats a state file that goes unrefreshed for
// SUSPEND_THRESHOLD (20s) as a suspend and starts a fresh cycle. Only a running
// `watch` refreshes that timestamp, so the watcher process has to stay up.

import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Hyprland
import qs.Commons
import qs.Ui

BarWidget {
  id: root
  moduleName: "tearhero"

  // ── Contract with tearhero ────────────────────────────────────────────
  // Phase always comes from the `class` field of `tearhero watch`, so it can
  // never drift from the binary. WORK_SECS is duplicated from main.rs purely
  // to scale the progress ring; if it ever changes there the ring fills at
  // the wrong rate, but the icon and phases stay correct.
  readonly property string binary: Quickshell.env("HOME") + "/.local/bin/tearhero"
  readonly property int workSecs: 20 * 60

  readonly property string iconClosed: "\u{f0209}"   // 󰈉 eye-off
  readonly property string iconOutline: "\u{f06d0}"  // 󰛐 eye-outline
  readonly property string iconFilled: "\u{f0208}"   // 󰈈 eye

  property string phase: "off"        // off | running | alert | rest
  property string mode: "normal"      // normal | silent
  property int secondsLeft: -1
  property string statusTooltip: ""

  // ── Fullscreen suppression ────────────────────────────────────────────
  // Hyprland reports fullscreen per workspace, and Quickshell surfaces the
  // raw IPC object. The binary makes the same check (via hyprctl) before it
  // plays a chime, so fullscreen mutes audio as well as dimming the alert.
  readonly property bool fullscreenActive: {
    var ws = Hyprland.focusedWorkspace
    if (!ws) return false
    var ipc = ws.lastIpcObject
    // hasfullscreen is briefly undefined while Quickshell populates the
    // workspace list, hence the coercion rather than a direct return.
    return !!(ipc && ipc.hasfullscreen)
  }

  // The break is only *hidden*, never skipped: the timer keeps running and
  // the alert reappears the moment the fullscreen window is left.
  readonly property bool suppressed: fullscreenActive && phase === "alert"
  readonly property bool alerting: phase === "alert" && !suppressed

  readonly property real progress: {
    if (phase !== "running" || secondsLeft < 0) return 0
    return Math.max(0, Math.min(1, 1 - (secondsLeft / workSecs)))
  }

  // Derived from `phase` and `fullscreenActive` directly rather than from
  // `alerting`, even though the two are equivalent. Several bindings depend on
  // `phase`, and QML does not order their re-evaluation, so routing the icon
  // through the intermediate `alerting` let it paint once with a stale value —
  // a one-frame outline flash on every transition into the alert.
  readonly property string icon: {
    if (phase === "off" || phase === "rest") return iconClosed
    if (phase === "alert" && !fullscreenActive) return iconFilled
    return iconOutline
  }

  // ── Commands ──────────────────────────────────────────────────────────
  // execDetached with an argv array: no shell, no quoting, nothing to block on.
  function run(args) {
    Quickshell.execDetached([root.binary].concat(args))
  }
  function restNow() { run(["rest"]) }
  function skip() { if (phase !== "off") run(["skip"]) }
  function turnOff() { run(["off"]) }
  // "off" | "normal" | "silent" — normal/silent also turn the timer on.
  function setMode(value) {
    if (value === "off") turnOff()
    else run(["mode", value])
  }

  function consume(line) {
    if (!line) return
    var data
    try {
      data = JSON.parse(line)
    } catch (e) {
      return
    }
    phase = String(data.class || "off")
    mode = String(data.mode || "normal")
    statusTooltip = String(data.tooltip || "")
    secondsLeft = (typeof data.left === "number") ? data.left : -1
  }

  Process {
    id: watcher
    command: [root.binary, "watch"]
    running: true
    stdout: SplitParser {
      onRead: function(line) { root.consume(line) }
    }
    // If the binary dies (rebuild, crash) fall back to "off" and retry rather
    // than leaving a stale phase frozen on the bar forever.
    onExited: {
      root.phase = "off"
      root.secondsLeft = -1
      restartTimer.start()
    }
  }

  Timer {
    id: restartTimer
    interval: 5000
    repeat: false
    onTriggered: watcher.running = true
  }

  // ── Panel plumbing (contract for shell.summon / hide / toggle and Bar routing)
  readonly property bool opened: panelLoader.item ? panelLoader.item.opened === true : false

  function open() {
    if (panelLoader.item) panelLoader.item.open()
  }

  function close() {
    if (panelLoader.item) panelLoader.item.close()
  }

  function togglePanel() {
    if (panelLoader.item) panelLoader.item.toggle()
  }

  readonly property bool popoutSwitchClosing: panelLoader.item ? panelLoader.item.popoutSwitchClosing === true : false

  function closeForPopoutSwitch() {
    if (panelLoader.item) panelLoader.item.closeForPopoutSwitch()
  }

  function injectPanel() {
    var target = panelLoader.item
    if (!target) return
    if ("bar" in target) target.bar = root.bar
    if ("settings" in target) target.settings = root.settings
    if ("anchorItem" in target) target.anchorItem = button
    if ("hostWidget" in target) target.hostWidget = root
  }

  onBarChanged: injectPanel()
  onSettingsChanged: injectPanel()

  Loader {
    id: panelLoader
    active: true
    source: Qt.resolvedUrl("Panel.qml")
    visible: false
    onLoaded: {
      root.injectPanel()
      Qt.callLater(root.injectPanel)
    }
  }

  IpcHandler {
    target: "tearhero"
    function toggle(): void { root.togglePanel() }
    function open(): void { root.open() }
    function close(): void { root.close() }
    function rest(): void { root.restNow() }
    function skip(): void { root.skip() }
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  // Refresh the text in place while the tooltip is already up. Calling
  // showTooltip() again would re-arm the bar's 400ms reveal timer, so a
  // once-a-second countdown would keep resetting it and never settle.
  onStatusTooltipChanged: {
    if (bar && bar.tooltipShown && bar.tooltipTarget === button)
      bar.tooltipText = tooltipFor()
  }

  function tooltipFor() {
    var base = statusTooltip !== "" ? statusTooltip : "20-20-20 timer off\nClick for options"
    if (suppressed)
      base = "Eye break due — held back while fullscreen\nIt returns when you leave fullscreen"
        + (mode === "silent" ? "\nSilent mode" : "")
    if (phase !== "off")
      base += "\nMiddle-click to skip this cycle"
    return base
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.icon
    fontSize: 13
    hasVisualContent: true
    active: root.alerting
    activeColor: root.bar ? root.bar.urgent : Color.urgent
    // Dim when off, and while a fullscreen window is hiding a due break, so
    // the widget still reads as "not demanding anything right now".
    dimmed: root.phase === "off" || root.suppressed
    tooltipText: root.tooltipFor()

    onPressed: function(b) {
      if (b === Qt.RightButton) {
        root.restNow()
      } else if (b === Qt.MiddleButton) {
        root.skip()
      } else {
        root.togglePanel()
      }
    }
  }

  // ── Progress ring ─────────────────────────────────────────────────────
  Canvas {
    id: ring
    anchors.centerIn: button
    width: 20
    height: 20
    z: button.z + 1
    opacity: root.phase === "running" ? 0.55 : 0
    visible: opacity > 0

    Behavior on opacity { NumberAnimation { duration: 200 } }

    onPaint: {
      var ctx = getContext("2d")
      ctx.reset()
      var cx = width / 2
      var cy = height / 2
      var r = (Math.min(width, height) / 2) - 1.5
      var base = button.foreground

      ctx.lineWidth = 1.5
      ctx.lineCap = "round"

      ctx.beginPath()
      ctx.arc(cx, cy, r, 0, Math.PI * 2)
      ctx.strokeStyle = Qt.rgba(base.r, base.g, base.b, 0.18)
      ctx.stroke()

      if (root.progress > 0) {
        ctx.beginPath()
        // Start at 12 o'clock and fill clockwise.
        ctx.arc(cx, cy, r, -Math.PI / 2, -Math.PI / 2 + Math.PI * 2 * root.progress)
        ctx.strokeStyle = base
        ctx.stroke()
      }
    }
  }

  // Canvas does not observe bound properties, so repaint explicitly.
  Connections {
    target: root
    function onProgressChanged() { ring.requestPaint() }
  }
  Connections {
    target: button
    function onForegroundChanged() { ring.requestPaint() }
  }

  // Smooth pulse instead of the 500ms icon swap the raw stream drives.
  // A standalone animation targeting `scale`, not a `SequentialAnimation on
  // scale` value source: the value source owns the property for the object's
  // whole life, so the imperative reset below was fighting it for control and
  // could leave the icon parked at an arbitrary size.
  SequentialAnimation {
    id: pulse
    running: root.alerting
    loops: Animation.Infinite
    NumberAnimation { target: button; property: "scale"; from: 1.0; to: 1.18; duration: 600; easing.type: Easing.InOutSine }
    NumberAnimation { target: button; property: "scale"; from: 1.18; to: 1.0; duration: 600; easing.type: Easing.InOutSine }
    onRunningChanged: if (!running) button.scale = 1.0
  }
}
