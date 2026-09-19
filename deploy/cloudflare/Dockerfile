FROM alpine:3.23@sha256:85fe1e81d6758c208f3e1eed4338a1997e19d4be002d4dd32d3100c9a8c010a0
RUN apk add --no-cache ca-certificates curl \
    && curl --fail --location --retry 3 \
       https://github.com/n0-computer/iroh/releases/download/v1.1.0/iroh-relay-v1.1.0-x86_64-unknown-linux-musl.tar.gz \
       -o /tmp/relay.tar.gz \
    && echo '9a68108b824e4164ad2eec729cf0e8167e4cb50581cb745a242bace135df7614  /tmp/relay.tar.gz' | sha256sum -c - \
    && tar -xzf /tmp/relay.tar.gz -C /usr/local/bin --strip-components=1 \
    && rm /tmp/relay.tar.gz \
    && apk del curl
COPY relay-entrypoint.sh /usr/local/bin/relay-entrypoint
RUN chmod 755 /usr/local/bin/relay-entrypoint && adduser -D relay
USER relay
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/relay-entrypoint"]
