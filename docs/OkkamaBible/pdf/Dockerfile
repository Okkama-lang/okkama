# The PDF toolchain of the Okkama Bible: pandoc turns the generated Markdown
# into HTML, WeasyPrint lays the HTML out as pages. Fonts cover Cyrillic.
FROM debian:12
RUN apt-get update && apt-get install -y --no-install-recommends \
        pandoc python3 python3-pip python3-venv poppler-utils \
        fonts-noto-core fonts-noto-mono fonts-dejavu-core fonts-jetbrains-mono \
        libpango-1.0-0 libpangoft2-1.0-0 libharfbuzz0b libffi8 shared-mime-info \
    && rm -rf /var/lib/apt/lists/*
RUN python3 -m venv /opt/wp && /opt/wp/bin/pip install --no-cache-dir weasyprint==62.3 pydyf==0.10.0
# Golos Text, the face of the book (SIL OFL), from the Google Fonts repository.
ADD https://raw.githubusercontent.com/google/fonts/main/ofl/golostext/GolosText%5Bwght%5D.ttf /usr/share/fonts/truetype/golos/GolosText.ttf
# EB Garamond (SIL OFL, with Cyrillic) for the illuminated initials.
ADD https://raw.githubusercontent.com/google/fonts/main/ofl/ebgaramond/EBGaramond%5Bwght%5D.ttf /usr/share/fonts/truetype/golos/EBGaramond.ttf
RUN chmod 644 /usr/share/fonts/truetype/golos/*.ttf && fc-cache -f
ENV PATH=/opt/wp/bin:$PATH
WORKDIR /book
