import QtQuick
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami

Kirigami.FormLayout {
    property alias cfg_showClaude: claude.checked
    property alias cfg_showCodex: codex.checked
    property alias cfg_executable: executable.text

    QQC2.CheckBox {
        id: claude
        Kirigami.FormData.label: i18n("Panel rows:")
        text: i18n("Claude")
    }
    QQC2.CheckBox { id: codex; text: i18n("Codex") }
    QQC2.TextField {
        id: executable
        Kirigami.FormData.label: i18n("Executable:")
        placeholderText: "~/.local/bin/agent-gauge (default)"
    }
    QQC2.Label {
        text: i18n("Use an absolute executable path, or leave blank for the default.\nProvider collection is controlled by the app’s config.json.")
        wrapMode: Text.WordWrap
    }
}
