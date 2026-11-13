// Run through org.kde.PlasmaShell.evaluateScript; leave existing widgets in order.
(function () {
    var plugin = "com.github.agentusage.panel";
    var all = panels();
    for (var p = 0; p < all.length; ++p) {
        var current = all[p].widgets();
        for (var w = 0; w < current.length; ++w) {
            if (current[w].type === plugin) {
                print("AgentGauge is already on panel " + all[p].id);
                return;
            }
        }
    }
    var top = all.filter(function (panel) { return panel.location === "top"; });
    if (!top.length) throw new Error("No top panel found. Add AgentGauge from KDE’s Add Widgets menu.");
    var target = top[0];
    var anchor = null;
    for (var t = 0; t < top.length; ++t) {
        var widgets = top[t].widgets();
        for (var i = 0; i < widgets.length; ++i) {
            if (widgets[i].type === "org.kde.plasma.kstats") {
                target = top[t];
                anchor = widgets[i];
                break;
            }
        }
        if (anchor) break;
    }
    var index = anchor ? anchor.index + 1 : target.widgets().length;
    var widget = target.addWidget(plugin);
    widget.index = index;
    print("Added AgentGauge to top panel " + target.id + " at index " + widget.index);
})();
