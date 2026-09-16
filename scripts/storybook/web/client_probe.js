// A deterministic, local-only replay of the gallery's existing demo controls.
// It uses the same public input dispatcher as the native fixture tests.
export async function runClientProbe(wasm, trace) {
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const frame = () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const action = (value) => wasm.action(JSON.stringify(value));
  const click = async (id) => {
    const elements = JSON.parse(wasm.snapshot()).elements;
    if (id.startsWith("liquid-tab-") && !elements.some((element) => element.id === id) && elements.some((element) => element.id === "liquid-library-toggle")) {
      action({ type: "click", target: { element_id: "liquid-library-toggle" } });
      await frame();
    }
    action({ type: "click", target: { element_id: id } });
    await frame();
  };
  trace.start();
  const compare = new URLSearchParams(location.search).get("msaa") === "compare";
  for (const pass of compare ? ["baseline-a", "discard", "baseline-b"] : [""]) {
    trace.phase(pass + "setup");
    window.liquidDiscardPathSamples = pass === "discard";
    wasm.select_story("liquid-gallery");
    await frame();
    await click("liquid-tab-4");
    await click("liquid-count-14");
    const state = JSON.parse(wasm.story_state());
    if (state.group !== 4 || state.benchmarkCount !== 14) throw new Error("The automatic probe did not enter the 14-family benchmark");
    await click("liquid-cycle");
    for (let section = 0; section < 10; section++) {
      trace.phase(pass + "section-" + section);
      await wait(compare ? 700 : 2200);
      action({ type: "scroll", target: { x: innerWidth / 2, y: innerHeight - 80 }, delta_y: -420 });
      await frame();
    }
  }
  window.liquidDiscardPathSamples = false;
  trace.phase("rest");
  wasm.select_story("liquid-gallery");
  await frame();
}
