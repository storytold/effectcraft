// Disable every selected layer (the "no glow" render N of glow_fit.py).
var ls = selectedLayers();
for (var k = 0; k < ls.length; k++) ls[k][1].enabled = false;
writeLn("glow off: " + ls.length + " layers disabled");
