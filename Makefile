# Сборка и установка Okkama (Rust-реализация).
#
#     make                 # = make build
#     make install         # сборка и установка в ~/.local (PREFIX=/opt/okkama make install),
#                          # вместе с расширением VS Code, если есть VS Code и npm
#     make uninstall
#     make test            # тесты Rust
#     make gate            # ворота релиза в контейнере: тесты, conformance, std (make gate-local — без контейнера)
#     make pgo             # release-сборка с PGO (нужен `rustup component add llvm-tools`)
#     make install-pgo     # PGO-сборка и установка
#     make dist            # пакеты этой платформы в dist/ (tar.gz + .pkg/.deb)
#     make vscode          # расширение VS Code (.vsix) и его установка в VS Code
#
# `build` собирает okkama, sparrow и blade; криптография встроена в Sparrow.

PREFIX ?= $(HOME)/.local
CARGO  ?= cargo
MAN_PAGES ?= 0
DOC_OUTPUT ?= $(CURDIR)/reference
# Optional source-derived man pages: make install MAN_PAGES=1.
MAN_OPTION := $(if $(filter 1,$(MAN_PAGES)),--man-pages,)

RS      := okkama-rs
DOC_BIN ?= $(if $(CARGO_TARGET_DIR),$(CARGO_TARGET_DIR),$(CURDIR)/$(RS)/target)/release/okkama
VSIX    := vscode-okkama/okkama-vscode.vsix

.PHONY: all build pgo install install-pgo uninstall test gate gate-local dist reference vscode editor clean

all: build

# A release build compiles the development instrumentation (okkama-devtrace, OKKAMA_DEV) out;
# `make build RELEASE_CFG=` keeps it.
# RUSTFLAGS replaces okkama-rs/.cargo/config.toml, so a release build repeats its frame pointers (#930).
RELEASE_CFG ?= --cfg okkama_release -C force-frame-pointers=yes

build:
	cd $(RS) && RUSTFLAGS="$$RUSTFLAGS $(RELEASE_CFG)" $(CARGO) build --release -p okkama-cli -p sparrow-cli -p blade-cli

install: build
	./install.sh --prefix "$(PREFIX)" --no-build $(MAN_OPTION)
	@$(MAKE) --no-print-directory editor

# Оптимизация по профилю (tools/pgo/build.sh): на 6–18% быстрее обычной сборки.
pgo:
	sh $(RS)/tools/pgo/build.sh

install-pgo: pgo
	./install.sh --prefix "$(PREFIX)" --no-build $(MAN_OPTION)
	@$(MAKE) --no-print-directory editor

uninstall:
	./install.sh --prefix "$(PREFIX)" --uninstall

test:
	cd $(RS) && $(CARGO) test --release --workspace

gate:
	./tools/gate/docker.sh

gate-local:
	./tools/gate/gate.sh

dist:
	./tools/package.sh --installer $(MAN_OPTION)

# Static reference for okkama-docs (all links are relative inside this directory).
reference: build
	OKKAMA_STDLIB_DIR="$(CURDIR)/std" "$(DOC_BIN)" man --all --format html --output "$(DOC_OUTPUT)"

$(VSIX): vscode-okkama/package.json vscode-okkama/extension.js vscode-okkama/syntaxes/okkama.tmLanguage.json vscode-okkama/language-configuration.json
	cd vscode-okkama && npm install --omit=dev --no-audit --no-fund
	cd vscode-okkama && npx --yes @vscode/vsce@3 package --skip-license --allow-missing-repository -o okkama-vscode.vsix < /dev/null

vscode: $(VSIX)
	@code="$$(command -v code || echo '/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code')"; \
	if [ -x "$$code" ]; then "$$code" --install-extension $(VSIX) --force \
		&& echo "==> reload every open VS Code window: Cmd+Shift+P, Developer: Reload Window"; \
	else echo "VS Code CLI not found: install $(VSIX) via Extensions: Install from VSIX"; fi

# The editor follows the language: an install also refreshes the VS Code
# extension when VS Code and npm are there, and never fails because of it.
editor:
	@code="$$(command -v code || echo '/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code')"; \
	if [ -x "$$code" ] && command -v npm >/dev/null 2>&1; then \
		$(MAKE) --no-print-directory vscode || echo "==> the VS Code extension was not installed; run make vscode"; \
	else echo "==> VS Code or npm not found: the editor extension is skipped (make vscode)"; fi

clean:
	cd $(RS) && $(CARGO) clean
	rm -f $(VSIX)
