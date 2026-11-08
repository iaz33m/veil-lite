// Apply the saved colour mode before first paint (an external file so the CSP needs no inline script).
try {
  var m = localStorage.getItem("veil.mode");
  if (m === "light" || m === "dark") document.documentElement.dataset.mode = m;
} catch (e) {}
