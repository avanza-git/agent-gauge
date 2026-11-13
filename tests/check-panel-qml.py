"""Compile the widget QML using the same installed Qt/KDE modules as Plasma."""
import os
from pathlib import Path
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
from PySide6.QtCore import QUrl
from PySide6.QtGui import QGuiApplication
from PySide6.QtQml import QQmlComponent, QQmlEngine
app = QGuiApplication([])
engine = QQmlEngine()
for directory in ("/usr/lib64/qt6/qml", "/usr/lib/qt6/qml", "/usr/lib/x86_64-linux-gnu/qt6/qml"):
    if Path(directory).is_dir():
        engine.addImportPath(directory)
failed = False
for path in sorted(Path("plasma").rglob("*.qml")):
    component = QQmlComponent(engine, QUrl.fromLocalFile(str(path.resolve())))
    if component.isError():
        failed = True
        for error in component.errors():
            print(error.toString())
    else:
        print(f"Compiled {path}")
raise SystemExit(1 if failed else 0)
