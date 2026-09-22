import type { Catalog, PageFixture, Story } from "../workbench/types";
type LegacyProfile = {
  id: string;
  profile_id: string;
  name: string;
  node: string;
  provider: string;
  billing: string;
  models: Array<Record<string, unknown>>;
  verified: boolean;
};
type LegacyPerson = {
  name: string;
  avatar: string;
  role: string;
  node: string;
  modelConnectionId?: string;
  modelId?: string;
};
declare const profiles: LegacyProfile[];
declare const people: Record<string, LegacyPerson>;
declare const devices: Record<string, { name: string; description: string; online: boolean }>;
declare const profileCatalog: Array<{ id: string; label: string; billing: Array<{ id: string; label: string }> }>;
declare const conversations: Array<{
  id: string;
  name: string;
  kind: string;
  leaderId: string;
  node: string;
  members: string[];
  unread: number;
  time: string;
  messages: Array<{ id: string; who: string; text: string; time: string }>;
}>;
declare let current: string;
declare function render(): void;
declare function setSidebarWidth(width: number, save?: boolean): void;
const one = <T extends Element = HTMLElement>(selector: string) => {
  const element = document.querySelector<T>(selector);
  if (!element) throw Error("HTML 参考缺少 " + selector);
  return element;
};
const click = (selector: string) => one<HTMLElement>(selector).click();
const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
async function setup(story: Story, fixture: PageFixture, providers: Catalog["providers"], base: string) {
  const style = document.createElement("style");
  style.textContent = `
 @font-face{font-family:ZorkCompareInter;src:url(${base}assets/fonts/InterVariable.ttf);font-weight:100 900}
 @font-face{font-family:ZorkCompareCJK;src:url(${base}comparison-fonts/NotoSansSC.ttf);font-weight:100 900}
 :root{--font-body:ZorkCompareInter,ZorkCompareCJK,sans-serif;--sidebar-width:${fixture.device.sidebar_width}px}
 body *{visibility:hidden!important}[data-reference-target],[data-reference-target] *{visibility:visible!important}dialog::backdrop{visibility:hidden!important}
 `;
  document.head.append(style);
  const id = fixture.profile.profile_id;
  profiles.splice(0, profiles.length, {
    id,
    profile_id: id,
    name: id,
    node: fixture.device.id,
    provider: fixture.profile.provider,
    billing: fixture.profile.billing,
    models: structuredClone(fixture.profile.models),
    verified: false,
  });
  for (const person of Object.values(people)) person.node = "mini2";
  for (const agent of fixture.agents)
    people[agent.prototype_id] = {
      name: agent.name,
      avatar: agent.avatar,
      role: agent.role === "leader" ? "Leader" : "Worker",
      node: fixture.device.id,
      modelConnectionId: id,
      modelId: agent.model,
    };
  devices[fixture.device.id].name = fixture.device.name;
  setSidebarWidth(fixture.device.sidebar_width, false);
  for (const provider of profileCatalog) {
    const actual = providers.find((p) => p.id === provider.id);
    if (!actual) continue;
    provider.label = actual.label;
    for (const billing of provider.billing) {
      const actualBilling = actual.billing.find((b) => b.id === billing.id);
      if (actualBilling) billing.label = actualBilling.label;
    }
  }
  const settings = (section: string) => {
    click('[data-action="settings"]');
    click(`[data-action="device-settings-page"][data-device="${fixture.device.id}"][data-page="${section}"]`);
  };
  const connection = () => {
    settings("models");
    click('[data-action="add-profile"]');
    click("#profile-provider-menu summary");
    click(`[data-action="profile-provider"][data-provider="${fixture.profile.provider}"]`);
  };
  const state = story.state.replace(/-(compact|wide)$/, ""),
    family = story.family;
  let selector = ".settings-content";
  if (family === "connection") {
    if (state === "list") settings("models");
    else {
      connection();
      selector = "#chat-dialog";
      if (state === "provider") click("#profile-provider-menu summary");
    }
  } else if (family === "model") {
    settings("models");
    click(`[data-action="view-profile"][data-profile="${id}"]`);
    selector = "#chat-dialog";
    if (state === "create") {
      click('[data-action="add-profile-model"]');
      click(".profile-capacity summary");
      one<HTMLInputElement>('[name="model"]').value = fixture.model_form.id;
      one<HTMLInputElement>('[name="context"]').value = String(fixture.model_form.context_window);
      one<HTMLInputElement>('[name="output"]').value = String(fixture.model_form.max_output_tokens);
      one<HTMLInputElement>('[name="model"]').focus();
    }
  } else if (family === "agent") {
    settings("agents");
    if (state !== "list") {
      click(
        state === "create"
          ? '[data-action="add-agent"]'
          : `[data-action="edit-agent"][data-agent="${fixture.agents[0].prototype_id}"]`,
      );
      selector = "#chat-dialog";
      if (state === "create") {
        const profile = one<HTMLSelectElement>("#agent-profile");
        profile.value = fixture.profile.profile_id;
        profile.dispatchEvent(new Event("change", { bubbles: true }));
        one<HTMLSelectElement>("#agent-model").value = fixture.agents[0].model;
      }
    }
  } else if (family === "conversation") {
    const agent = fixture.agents[0];
    for (const id of Object.keys(devices)) if (id !== fixture.device.id) delete devices[id];
    conversations.splice(0, conversations.length, {
      id: fixture.conversation.id,
      name: agent.name,
      kind: "leader",
      leaderId: agent.prototype_id,
      node: fixture.device.id,
      members: [agent.prototype_id],
      unread: 0,
      time: "",
      messages: fixture.conversation.messages.map((message) => ({
        id: message.id,
        who: message.who,
        text: message.content,
        time: message.time,
      })),
    });
    current = fixture.conversation.id;
    render();
    one<HTMLTextAreaElement>("#message-form textarea").placeholder = fixture.conversation.placeholder;
    selector = state === "composer" ? ".composer" : ".chat-app";
  }
  const measure = () => {
    const target =
      document.querySelector<HTMLDialogElement>("#chat-dialog[open]") ??
      document.querySelector(selector === "#chat-dialog" ? ".settings-content" : selector);
    if (!target) return;
    document.querySelectorAll("[data-reference-target]").forEach((element) => {
      if (element !== target) element.removeAttribute("data-reference-target");
    });
    target.setAttribute("data-reference-target", "");
    const bounds = target.getBoundingClientRect();
    parent.postMessage(
      {
        type: "zork-reference-bounds",
        id: story.id,
        surface: target.tagName === "DIALOG" ? "dialog" : "page",
        bounds: { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height },
      },
      location.origin,
    );
  };
  await document.fonts.ready;
  await Promise.all([...document.images].map((image) => image.decode().catch(() => undefined)));
  await frame();
  await frame();
  measure();
  new MutationObserver(() => requestAnimationFrame(measure)).observe(document.body, {
    subtree: true,
    childList: true,
    attributes: true,
    attributeFilter: ["open"],
  });
  new ResizeObserver(measure).observe(document.body);
  document.documentElement.dataset.referenceReady = story.id;
  document.documentElement.dataset.fixture = JSON.stringify({
    device: fixture.device.id,
    profile: id,
    model: fixture.profile.models[0]?.id,
    agent: fixture.agents[0]?.name,
    width: innerWidth,
    height: innerHeight,
  });
}
window.addEventListener("message", (event) => {
  if (event.source !== parent || event.origin !== location.origin || event.data?.type !== "zork-reference-setup")
    return;
  const { story, fixture, providers, base } = event.data as {
    story: Story;
    fixture: PageFixture;
    providers: Catalog["providers"];
    base: string;
  };
  void setup(story, fixture, providers, base).catch((error) =>
    parent.postMessage(
      { type: "zork-reference-error", id: story.id, message: error instanceof Error ? error.message : String(error) },
      location.origin,
    ),
  );
});
