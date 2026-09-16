// Test-only GPU timestamps. The application renderer and its workload are unchanged.
// Keep Chrome's default timestamp quantization; do not enable developer/unsafe flags.
(() => {
  window.liquidTimestampRows = [];
  window.liquidTimestampErrors = [];
  if (!window.GPUAdapter || !window.GPUQueue) return;
  const restores = [],
    resources = [],
    pending = new Set(),
    flushers = [];
  const submitted = new WeakMap();
  const requestDevice = GPUAdapter.prototype.requestDevice;
  GPUAdapter.prototype.requestDevice = async function (descriptor = {}) {
    if (!this.features.has("timestamp-query")) {
      window.liquidTimestampErrors.push("timestamp-query is unavailable");
      return requestDevice.call(this, descriptor);
    }
    const device = await requestDevice.call(this, {
      ...descriptor,
      requiredFeatures: [...new Set([...(descriptor.requiredFeatures || []), "timestamp-query"])],
    });
    const count = 1024,
      bytes = count * 8,
      free = [],
      readbacks = [];
    // Batch readback to avoid a mapAsync round trip per command buffer.
    // Every render-pass timestamp and frame identity is still retained.
    const batchSize = 8;
    let batch;
    const readBatch = (capture) => {
      if (!capture.sealed || capture.reading || capture.rows.some((row) => !row.submitted)) return;
      capture.reading = true;
      const readback = capture.read
        .mapAsync(GPUMapMode.READ)
        .then(() => {
          try {
            const range = capture.read.getMappedRange();
            for (const { used, frame, phase, offset } of capture.rows) {
              const times = new BigUint64Array(range, offset, used);
              if (times[used - 1] < times[0]) throw new Error("GPU timestamp counter reset");
              if (phase === window.liquidTimestampPhase)
                window.liquidTimestampRows.push({
                  frame: frame.id,
                  at: frame.at,
                  passes: used / 2,
                  startNs: times[0].toString(),
                  endNs: times[used - 1].toString(),
                });
            }
            const limit = window.liquidTimestampLimit;
            if (limit && window.liquidTimestampRows.length > limit) {
              window.liquidTimestampRows.splice(0, window.liquidTimestampRows.length - limit);
            }
          } catch (error) {
            window.liquidTimestampErrors.push(String(error));
          } finally {
            capture.read.unmap();
            for (const row of capture.rows) free.push(row.resource);
            readbacks.push(capture.read);
          }
        })
        .catch((error) => window.liquidTimestampErrors.push(String(error)))
        .finally(() => pending.delete(readback));
      pending.add(readback);
    };
    const sealBatch = () => {
      if (!batch) return;
      batch.sealed = true;
      readBatch(batch);
      batch = undefined;
    };
    flushers.push(sealBatch);
    const reserveRead = (row) => {
      if (!batch) {
        const read =
          readbacks.pop() ||
          device.createBuffer({
            size: bytes * batchSize,
            usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
          });
        if (!resources.includes(read)) resources.push(read);
        batch = { read, rows: [], sealed: false, reading: false };
      }
      const capture = batch;
      row.offset = capture.rows.length * bytes;
      capture.rows.push(row);
      row.submit = () => {
        row.submitted = true;
        readBatch(capture);
      };
      if (capture.rows.length === batchSize) sealBatch();
      return capture.read;
    };
    let allocated = 0;
    const take = () => {
      if (free.length) return free.pop();
      if (allocated >= 64) {
        window.liquidTimestampErrors.push("timestamp readback pool exhausted");
        return null;
      }
      allocated++;
      const resource = {
        queries: device.createQuerySet({ type: "timestamp", count }),
        resolve: device.createBuffer({ size: bytes, usage: GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC }),
      };
      resources.push(resource);
      return resource;
    };
    const originalCreateEncoder = device.createCommandEncoder;
    const createEncoder = originalCreateEncoder.bind(device);
    device.createCommandEncoder = (descriptor) => {
      const encoder = createEncoder(descriptor);
      const frame = window.liquidCurrentRaf;
      if (!window.liquidObserve || !frame || !Number.isFinite(frame.at)) return encoder;
      const resource = take();
      if (!resource) return encoder;
      let used = 0;
      const begin = encoder.beginRenderPass.bind(encoder);
      encoder.beginRenderPass = (descriptor) => {
        if (descriptor.timestampWrites || used + 2 > count) {
          window.liquidTimestampErrors.push("timestamp pass coverage is incomplete");
          return begin(descriptor);
        }
        const pass = begin({
          ...descriptor,
          timestampWrites: {
            querySet: resource.queries,
            beginningOfPassWriteIndex: used,
            endOfPassWriteIndex: used + 1,
          },
        });
        used += 2;
        return pass;
      };
      const finish = encoder.finish.bind(encoder);
      encoder.finish = (descriptor) => {
        const capture = { resource, used, frame, phase: window.liquidTimestampPhase };
        if (used) {
          const read = reserveRead(capture);
          encoder.resolveQuerySet(resource.queries, 0, used, resource.resolve, 0);
          encoder.copyBufferToBuffer(resource.resolve, 0, read, capture.offset, used * 8);
        }
        const buffer = finish(descriptor);
        if (used) submitted.set(buffer, capture);
        else free.push(resource);
        return buffer;
      };
      return encoder;
    };
    const wrappedCreateEncoder = device.createCommandEncoder;
    restores.push(() => {
      if (device.createCommandEncoder === wrappedCreateEncoder) device.createCommandEncoder = originalCreateEncoder;
    });
    return device;
  };
  const wrappedRequestDevice = GPUAdapter.prototype.requestDevice;
  restores.push(() => {
    if (GPUAdapter.prototype.requestDevice === wrappedRequestDevice) GPUAdapter.prototype.requestDevice = requestDevice;
  });
  const submit = GPUQueue.prototype.submit;
  GPUQueue.prototype.submit = function (commands) {
    const buffers = Array.from(commands);
    const result = submit.call(this, buffers);
    for (const buffer of buffers) {
      const capture = submitted.get(buffer);
      if (!capture) continue;
      submitted.delete(buffer);
      capture.submit();
    }
    return result;
  };
  const wrappedSubmit = GPUQueue.prototype.submit;
  restores.push(() => {
    if (GPUQueue.prototype.submit === wrappedSubmit) GPUQueue.prototype.submit = submit;
  });
  window.liquidTimestampFlush = async () => {
    flushers.forEach((flush) => flush());
    await Promise.allSettled([...pending]);
  };
  window.liquidTimestampCleanup = async () => {
    window.liquidObserve = false;
    restores.reverse().forEach((restore) => restore());
    await window.liquidTimestampFlush();
    for (const resource of resources) {
      if (resource.queries) {
        resource.queries.destroy();
        resource.resolve.destroy();
      } else resource.destroy();
    }
    resources.length = 0;
  };
})();
