# The PDF toolchain of the Okkama Bible: pandoc turns the generated Markdown
# into HTML, WeasyPrint lays the HTML out as pages. Fonts cover Cyrillic.
FROM debian:12
RUN apt-get update && apt-get install -y --no-install-recommends \
        pandoc python3 python3-pip python3-venv poppler-utils \
        fonts-noto-core fonts-noto-mono fonts-dejavu-core fonts-jetbrains-mono \
        libpango-1.0-0 libpangoft2-1.0-0 libharfbuzz0b libffi8 shared-mime-info \
    && rm -rf /var/lib/apt/lists/*
RUN python3 -m venv /opt/wp && /opt/wp/bin/pip install --no-cache-dir weasyprint==62.3 pydyf==0.10.0
ENV PATH=/opt/wp/bin:$PATH
WORKDIR /book
