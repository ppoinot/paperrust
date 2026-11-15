# Official PdfCraft web build. The zip is downloaded at image build and checked.
# It is not vendored into this repository.
FROM docker.io/library/nginx:1.27-alpine

ARG PDFCRAFT_VERSION=0.21.1
ARG PDFCRAFT_SHA256=fe9708d46af191aebb56f70eb32cf8b112b0b9452d17ae6ba3a8c4a8665fe748

RUN apk add --no-cache --virtual .fetch curl unzip \
    && curl -fsSL -o /tmp/pdfcraft.zip \
        "https://github.com/storytold/pdfcraft/releases/download/v${PDFCRAFT_VERSION}/pdfcraft-web-${PDFCRAFT_VERSION}.zip" \
    && printf '%s  %s\n' "${PDFCRAFT_SHA256}" /tmp/pdfcraft.zip | sha256sum -c - \
    && unzip -q /tmp/pdfcraft.zip -d /tmp/pdfcraft \
    && rm -rf /usr/share/nginx/html/* \
    && cp -a "/tmp/pdfcraft/pdfcraft-web-${PDFCRAFT_VERSION}/." /usr/share/nginx/html/ \
    && rm -rf /tmp/pdfcraft /tmp/pdfcraft.zip \
    && apk del .fetch

COPY nginx.conf /etc/nginx/conf.d/default.conf
