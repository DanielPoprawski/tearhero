// TearHero popup panel: status, countdown, Off / Normal / Silent, rest, skip.
//
// All live state is bound to the host BarWidget (phase, mode, secondsLeft),
// which is the one place the `tearhero watch` stream is consumed. The panel
// never talks to the binary directly except through the host's helpers.

import QtQuick
import Quickshell
import qs.Commons
import qs.Ui

Panel {
  id: root
  moduleName: "tearhero"
  ipcTarget: "tearhero"
  manageIpc: false   // the BarWidget owns the IpcHandler

  property var anchorItem: null
  property var hostWidget: null   // injected by BarWidget.injectPanel
  readonly property var host: hostWidget || root

  readonly property color fg: bar ? bar.foreground : Color.foreground
  readonly property string ff: bar ? bar.fontFamily : Style.font.family

  // Fallbacks so bindings are valid before hostWidget is injected.
  readonly property string phase: host && host.phase !== undefined ? host.phase : "off"
  readonly property string mode: host && host.mode !== undefined ? host.mode : "normal"
  readonly property int secondsLeft: host && host.secondsLeft !== undefined ? host.secondsLeft : -1
  readonly property bool suppressed: host && host.suppressed === true
  readonly property bool alerting: host && host.alerting === true
  readonly property string icon: host && host.icon !== undefined ? host.icon : "\u{f0209}"

  readonly property string modeValue: phase === "off" ? "off" : mode

  function fmt(s) {
    if (s < 0) return ""
    return Math.floor(s / 60) + ":" + ("0" + (s % 60)).slice(-2)
  }

  readonly property string title: {
    if (phase === "running") return "Next eye break"
    if (phase === "alert") return "Eye break due"
    if (phase === "rest") return "Resting your eyes"
    return "Timer off"
  }

  readonly property string meta: {
    if (suppressed) return "HELD BACK WHILE FULLSCREEN"
    if (phase === "alert") return "LOOK 20 FEET AWAY FOR 20 SECONDS"
    if (phase === "off") return "20-20-20"
    return mode === "silent" ? "SILENT  •  NO CHIMES" : "NORMAL  •  CHIMES EVERY 3 MIN WHEN DUE"
  }

  function setMode(value) { if (host && host.setMode) host.setMode(value) }
  function restNow() { if (host && host.restNow) host.restNow() }
  function skip() { if (host && host.skip) host.skip() }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.hostWidget || root
    bar: root.bar
    open: root.opened
    centerOnBar: false
    focusTarget: keys
    contentWidth: panel.fittedContentWidth(Style.space(320))
    contentHeight: panel.fittedContentHeight(col.implicitHeight)

    PanelKeyCatcher {
      id: keys
      anchors.fill: parent

      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onActivateRequested: root.restNow()
      onTextKey: function(t) {
        var k = t.toLowerCase()
        if (k === "r") root.restNow()
        else if (k === "s") root.skip()
        else if (k === "o") root.setMode("off")
        else if (k === "n") root.setMode("normal")
        else if (k === "q") root.setMode("silent")
      }

      Column {
        id: col
        width: parent.width
        spacing: Style.space(12)

        PanelHero {
          width: parent.width
          title: root.title
          meta: root.meta
          detail: (root.phase === "running" || root.phase === "rest") ? root.fmt(root.secondsLeft) : ""
          foreground: root.fg
          fontFamily: root.ff
          iconComponent: Text {
            text: root.icon
            color: root.alerting ? Color.urgent : root.fg
            font.family: root.ff
            font.pixelSize: Style.font.display
          }
        }

        PanelSeparator { width: parent.width; foreground: root.fg }

        PanelSectionHeader {
          width: parent.width
          text: "MODE"
          foreground: root.fg
          fontFamily: root.ff
        }

        ButtonGroup {
          width: parent.width
          options: [
            { value: "off", label: "Off", icon: "\u{f0209}", tooltip: "Stop the timer (O)" },
            { value: "normal", label: "Normal", icon: "\u{f06d0}", tooltip: "Chime when a break is due, every 3 min until you rest (N)" },
            { value: "silent", label: "Silent", icon: "\u{f0581}", tooltip: "Same timer, no chimes (Q)" }
          ]
          value: root.modeValue
          foreground: root.fg
          accent: Color.accent
          fontFamily: root.ff
          fontSize: Style.font.bodySmall
          onChanged: function(v) { root.setMode(v) }
        }

        PanelSectionHeader {
          width: parent.width
          text: "ACTIONS"
          foreground: root.fg
          fontFamily: root.ff
        }

        Row {
          width: parent.width
          spacing: Style.space(8)

          Button {
            text: "Rest now"
            iconText: "\u{f0209}"
            bordered: true
            tooltipText: "Start the 20 second rest (R)"
            enabled: root.phase === "running" || root.phase === "alert"
            opacity: enabled ? 1.0 : 0.4
            foreground: root.fg
            accent: Color.accent
            fontFamily: root.ff
            onClicked: root.restNow()
          }

          Button {
            text: "Skip cycle"
            iconText: "\u{f04e6}"
            bordered: true
            tooltipText: "Restart the 20 minute timer without resting (S)"
            enabled: root.phase !== "off"
            opacity: enabled ? 1.0 : 0.4
            foreground: root.fg
            accent: Color.accent
            fontFamily: root.ff
            onClicked: root.skip()
          }
        }

        Text {
          width: parent.width
          text: "Fullscreen windows mute chimes and hide the alert until you leave them."
          color: Qt.darker(root.fg, 1.4)
          font.family: root.ff
          font.pixelSize: Style.font.caption
          wrapMode: Text.WordWrap
        }
      }
    }
  }
}
