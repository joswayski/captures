import { eventTargetBelongsToSelectIn } from "./customSelectMenu";

describe("eventTargetBelongsToSelectIn", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("recognizes a portaled listbox owned by a trigger in the container", () => {
    document.body.innerHTML = `
      <div id="panel">
        <button aria-controls="blend-listbox">Blend mode</button>
      </div>
      <div id="blend-listbox" class="custom-select-listbox" role="listbox">
        <button type="button">Multiply</button>
      </div>
    `;
    const panel = document.getElementById("panel");
    const option = document.querySelector("#blend-listbox button");
    expect(eventTargetBelongsToSelectIn(panel, option)).toBe(true);
  });

  it("ignores a listbox owned by a different container", () => {
    document.body.innerHTML = `
      <div id="panel"><span>empty</span></div>
      <button aria-controls="other-listbox">Format</button>
      <div id="other-listbox" class="custom-select-listbox" role="listbox">
        <button type="button">PNG</button>
      </div>
    `;
    const panel = document.getElementById("panel");
    const option = document.querySelector("#other-listbox button");
    expect(eventTargetBelongsToSelectIn(panel, option)).toBe(false);
  });
});
