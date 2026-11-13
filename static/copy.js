// Copy buttons for the values worth pasting elsewhere: the pull commands, image references and
// digests, and WIT. Each `[data-copy]` element gets a button that copies its text, or the value of
// its `data-copy` attribute when it has one. A block, e.g. a `<pre>`, has its button in its upper
// right corner, outside its scrolling; an inline value has its button after it. Without this
// script the values are still there to select.
(() => {
  const targets = [...document.querySelectorAll("[data-copy]")];
  if (!targets.length) return;

  const icon = (name) =>
    `<svg class="copy-icon copy-icon-${name}" aria-hidden="true" focusable="false"><use href="#meta-${name}"/></svg>`;

  // announces a copy to a screen reader
  const status = document.createElement("span");
  status.className = "visually-hidden";
  status.setAttribute("role", "status");
  document.body.append(status);

  const write = async (text) => {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // a browser without the clipboard API, or a page that isn't served securely
      const area = document.createElement("textarea");
      area.value = text;
      area.setAttribute("readonly", "");
      area.style.position = "fixed";
      area.style.opacity = "0";
      document.body.append(area);
      area.select();
      const copied = document.execCommand("copy");
      area.remove();
      return copied;
    }
  };

  for (const target of targets) {
    const what = target.dataset.copyLabel || "Copy";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "copy";
    button.title = what;
    button.setAttribute("aria-label", what);
    button.innerHTML = icon("copy") + icon("check");

    let reset = 0;
    button.addEventListener("click", async () => {
      const text = target.dataset.copy || target.textContent.trim();
      const copied = await write(text);
      button.classList.toggle("copied", copied);
      status.textContent = copied ? "Copied" : "Couldn't copy";
      clearTimeout(reset);
      reset = setTimeout(() => {
        button.classList.remove("copied");
        status.textContent = "";
      }, 1500);
    });

    if (getComputedStyle(target).display === "block" && target.tagName === "PRE") {
      const wrap = document.createElement("div");
      wrap.className = "copy-wrap";
      target.replaceWith(wrap);
      wrap.append(target, button);
    } else {
      target.after(button);
      target.parentElement.classList.add("copy-line");
    }
  }
})();
