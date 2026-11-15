# Official WordCraft web build. The zip is downloaded at image build and checked.
# It is not vendored into this repository.
FROM docker.io/library/nginx:1.27-alpine

ARG WORDCRAFT_VERSION=0.18.1
ARG WORDCRAFT_SHA256=3daa7c2d1f209a7eeb2527fcefa3fe31e875848fb1970d68f54edae55fe0bc9f

RUN apk add --no-cache --virtual .fetch curl unzip \
    && curl -fsSL -o /tmp/wordcraft.zip \
        "https://github.com/storytold/wordcraft/releases/download/v${WORDCRAFT_VERSION}/wordcraft-web-${WORDCRAFT_VERSION}.zip" \
    && printf '%s  %s\n' "${WORDCRAFT_SHA256}" /tmp/wordcraft.zip | sha256sum -c - \
    && unzip -q /tmp/wordcraft.zip -d /tmp/wordcraft \
    && rm -rf /usr/share/nginx/html/* \
    && cp -a "/tmp/wordcraft/wordcraft-web-${WORDCRAFT_VERSION}/." /usr/share/nginx/html/ \
    && rm -rf /tmp/wordcraft /tmp/wordcraft.zip \
    && apk del .fetch

COPY nginx.conf /etc/nginx/conf.d/default.conf
