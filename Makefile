# SPDX-FileCopyrightText: Björn Busse <bj.rn@baerlin.eu>
# SPDX-License-Identifier: BSD-3-Clause

ENGINE ?= podman

.PHONY: all swlc clean help

all: swlc

swlc: Containerfile src/main.rs src/lib.rs ## build swlc binary in Alpine 3.24 (static musl)
	$(ENGINE) build -f Containerfile -t swlc-builder:local .
	$(ENGINE) rm -f swlc-extract 2>/dev/null || true
	$(ENGINE) create --name swlc-extract swlc-builder:local
	$(ENGINE) cp swlc-extract:/build/target/release/swlc ./swlc
	$(ENGINE) rm swlc-extract

clean: ## remove build image and binary
	-$(ENGINE) rmi -f swlc-builder:local 2>/dev/null
	-rm -f swlc

help: ## list targets
	@grep -hE '^[a-z][a-z0-9 -]*:.*##' $(MAKEFILE_LIST) | sed 's/:.*##/\t/' | expand -t20
