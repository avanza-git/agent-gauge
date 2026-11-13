import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PC3
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasma5support as Plasma5Support
import org.kde.plasma.plasmoid
import "../code/usage.js" as Usage

PlasmoidItem {
    id: root
    property var snapshot: null
    property string readError: ""
    property bool inFlight: false
    property double now: Date.now() / 1000
    readonly property bool stale: Usage.stale(snapshot, now)
    readonly property var rows: Usage.providers(snapshot, Plasmoid.configuration.showClaude, Plasmoid.configuration.showCodex)
    readonly property string statusText: readError || (snapshot ? (stale ? i18n("Readings are stale. Check the AgentGauge service.") : i18n("Updated %1", new Date(snapshot.updated_at * 1000).toLocaleTimeString())) : i18n("Waiting for the AgentGauge service…"))

    Plasmoid.icon: "utilities-system-monitor"
    preferredRepresentation: compactRepresentation
    toolTipMainText: i18n("AgentGauge")
    toolTipSubText: statusText

    function refresh() {
        if (inFlight) return;
        inFlight = true;
        source.connectSource(Usage.command(Plasmoid.configuration.executable));
    }
    function rowColor(provider) {
        if (stale || provider.error || !provider.enabled) return Kirigami.Theme.disabledTextColor;
        var level = Usage.severity(provider, snapshot ? snapshot.thresholds : [], now);
        return level === 2 ? Kirigami.Theme.negativeTextColor : level === 1 ? Kirigami.Theme.neutralTextColor : Kirigami.Theme.textColor;
    }

    Plasma5Support.DataSource {
        id: source
        engine: "executable"
        connectedSources: []
        onNewData: function(sourceName, data) {
            source.disconnectSource(sourceName);
            root.inFlight = false;
            if (data["exit code"] !== 0) {
                root.readError = i18n("Snapshot unavailable. Check that agent-gauge.service is running and the executable path is correct.");
                return;
            }
            try {
                var next = JSON.parse(data.stdout);
                if (!Usage.validSnapshot(next)) throw new Error("Unsupported snapshot");
                root.snapshot = next;
                root.readError = "";
            } catch (error) {
                root.readError = i18n("Cannot read usage data: %1", String(error));
            }
        }
    }
    Timer {
        interval: 5000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: { root.now = Date.now() / 1000; root.refresh(); }
    }
    Connections {
        target: Plasmoid.configuration
        function onExecutableChanged() { root.refresh(); }
    }
    Plasmoid.contextualActions: [
        PlasmaCore.Action {
            text: i18n("Refresh display")
            icon.name: "view-refresh"
            onTriggered: root.refresh()
        }
    ]

    compactRepresentation: MouseArea {
        id: compact
        implicitWidth: Math.max(170, column.implicitWidth + 12)
        implicitHeight: 36
        Layout.minimumWidth: implicitWidth
        Layout.preferredWidth: implicitWidth
        Layout.minimumHeight: 24
        acceptedButtons: Qt.LeftButton
        onClicked: root.expanded = !root.expanded
        Column {
            id: column
            anchors.centerIn: parent
            spacing: 0
            Repeater {
                model: root.rows
                PC3.Label {
                    required property var modelData
                    text: modelData.label + "  " + (modelData.enabled ? "5h " + Usage.percent(modelData.session) + " · 7d " + Usage.percent(modelData.weekly) : i18n("off"))
                    textFormat: Text.PlainText
                    font.pixelSize: Math.max(9, Math.min(12, (compact.height - 4) / Math.max(1, root.rows.length) * 0.72))
                    color: root.rowColor(modelData)
                }
            }
            PC3.Label {
                visible: root.rows.length === 0
                text: i18n("AgentGauge")
            }
        }
    }

    fullRepresentation: ColumnLayout {
        Layout.minimumWidth: 350
        Layout.preferredWidth: 390
        Layout.minimumHeight: implicitHeight + 24
        Layout.preferredHeight: implicitHeight + 24
        spacing: Kirigami.Units.smallSpacing

        PC3.Label {
            text: i18n("AgentGauge")
            font.bold: true
            font.pointSize: Kirigami.Theme.defaultFont.pointSize * 1.2
        }
        Repeater {
            model: root.rows
            ColumnLayout {
                id: provider
                required property var modelData
                Layout.fillWidth: true
                PC3.Label {
                    text: provider.modelData.label + (provider.modelData.enabled ? "" : " — " + i18n("disabled in config.json"))
                    font.bold: true
                    color: root.rowColor(provider.modelData)
                }
                Repeater {
                    model: [ {label: "5-hour", window: provider.modelData.session}, {label: "Weekly", window: provider.modelData.weekly} ]
                    ColumnLayout {
                        id: limit
                        required property var modelData
                        Layout.fillWidth: true
                        RowLayout {
                            Layout.fillWidth: true
                            PC3.Label { text: limit.modelData.label + "  " + Usage.percent(limit.modelData.window) }
                            Item { Layout.fillWidth: true }
                            PC3.Label {
                                visible: text.length > 0
                                text: Usage.forecastText(limit.modelData.window, root.now)
                                color: Kirigami.Theme.neutralTextColor
                            }
                            PC3.Label { text: Usage.resetText(limit.modelData.window, root.now); opacity: 0.7 }
                        }
                        PC3.ProgressBar {
                            Layout.fillWidth: true
                            from: 0
                            to: 1
                            value: limit.modelData.window ? Math.min(1, Math.max(0, limit.modelData.window.utilization)) : 0
                            opacity: root.stale || provider.modelData.error ? 0.5 : 1
                        }
                    }
                }
                PC3.Label {
                    Layout.fillWidth: true
                    visible: text.length > 0
                    text: provider.modelData.error
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    color: Kirigami.Theme.neutralTextColor
                }
            }
        }
        PC3.Label {
            Layout.fillWidth: true
            text: root.statusText
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            opacity: 0.7
        }
        PC3.Label {
            Layout.fillWidth: true
            visible: root.stale && root.snapshot !== null
            text: i18n("Showing last-known readings.")
            color: Kirigami.Theme.neutralTextColor
        }
    }
}
