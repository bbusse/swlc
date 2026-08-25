# SPDX-FileCopyrightText: Björn Busse <bj.rn@baerlin.eu>
# SPDX-License-Identifier: BSD-3-Clause

BIN = target/release/swlc
ENGINE ?= podman
HASH   := $(shell git rev-parse --short HEAD)
REMOTE ?= gh
RELEASE_BRANCH ?= ci

.PHONY: all swlc build strip clean help \
        release release-candidate rc \
        _check-remote _check-branch _check-up-to-date

all: swlc

swlc: Containerfile src/main.rs src/lib.rs ## build swlc binary in Alpine 3.24 (static musl)
	$(ENGINE) build -f Containerfile -t swlc-builder:local .
	$(ENGINE) rm -f swlc-extract 2>/dev/null || true
	$(ENGINE) create --name swlc-extract swlc-builder:local
	$(ENGINE) cp swlc-extract:/build/target/release/swlc ./swlc
	$(ENGINE) rm swlc-extract

build: ## build natively with cargo, no container
	cargo build --release

strip: build ## strip the natively built binary
	strip $(BIN)

clean: ## remove build image, cargo artifacts and binary
	-$(ENGINE) rmi -f swlc-builder:local 2>/dev/null
	-rm -f swlc
	cargo clean

_check-remote:
	@git remote get-url $(REMOTE) > /dev/null 2>&1 || \
	    { echo "Error: no remote '$(REMOTE)' — add one with: git remote add $(REMOTE) <url>"; exit 1; }

_check-branch:
	@current="$$(git rev-parse --abbrev-ref HEAD)"; \
	if [ "$$current" != "$(RELEASE_BRANCH)" ]; then \
	    echo "Error: on branch '$$current' — releases must be tagged from '$(RELEASE_BRANCH)'. Checkout $(RELEASE_BRANCH) first."; \
	    exit 1; \
	fi

_check-up-to-date: _check-remote _check-branch
	@git fetch $(REMOTE) $(RELEASE_BRANCH) > /dev/null 2>&1
	@git merge-base --is-ancestor $(REMOTE)/$(RELEASE_BRANCH) HEAD || \
	    { echo "Error: $(RELEASE_BRANCH) has commits you don't have — pull/rebase before tagging a release."; exit 1; }

release: _check-up-to-date ## tag and push a full release
	$(eval TAG := release-$(HASH))
	git tag -f $(TAG)
	@printf 'Tagged %s as %s\n' "$(HASH)" "$(TAG)"
	@printf 'Push tag to trigger a release? [y/N] ' && read ans && \
	    case "$$ans" in [yY]) git push $(REMOTE) $(TAG) ;; \
	    *) git tag -d $(TAG); echo 'Aborted — tag removed.' ;; esac

rc: release-candidate ## alias for release-candidate

release-candidate: _check-up-to-date ## tag and push a release candidate
	$(eval TAG := rc-$(HASH))
	git tag -f $(TAG)
	@printf 'Tagged %s as %s\n' "$(HASH)" "$(TAG)"
	@printf 'Push tag to trigger a release candidate? [y/N] ' && read ans && \
	    case "$$ans" in [yY]) git push $(REMOTE) $(TAG) ;; \
	    *) git tag -d $(TAG); echo 'Aborted — tag removed.' ;; esac

help: ## list targets
	@grep -hE '^[a-z][a-z0-9 -]*:.*##' $(MAKEFILE_LIST) | sed 's/:.*##/\t/' | expand -t20
