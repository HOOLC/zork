// Diagnostic A/B switch only. Resolve consumes MSAA samples before discard.
// The resolved image and the path's sample count remain unchanged.
const prototype = window.GPUCommandEncoder?.prototype;
if (prototype) {
  const begin = prototype.beginRenderPass;
  window.liquidPathDiscarded = 0;
  const wrapped = function (descriptor) {
    if (descriptor.label === "path_rasterization_pass") {
      const colorAttachments = descriptor.colorAttachments.map((attachment) => {
        if (!attachment?.resolveTarget || attachment.loadOp !== "clear") return attachment;
        const storeOp = window.liquidDiscardPathSamples ? "discard" : "store";
        if (storeOp === "discard") window.liquidPathDiscarded++;
        return { ...attachment, storeOp };
      });
      descriptor = { ...descriptor, colorAttachments };
    }
    return begin.call(this, descriptor);
  };
  prototype.beginRenderPass = wrapped;
  window.liquidPathStoreCleanup = () => {
    window.liquidDiscardPathSamples = false;
    if (prototype.beginRenderPass === wrapped) prototype.beginRenderPass = begin;
  };
}
