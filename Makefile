# vlfo — build and run helpers.
#
#   make            build (release)
#   make run        live window with the test shader, OSC on udp 9000
#   make pd         open the Pd control patch (PATCH, default pd/lfo-test.pd)
#   make live       pd + run together; Ctrl-C stops both
#                   e.g. make live SHADER=shaders/squares.fs PATCH=pd/squares.pd
#   make render     headless PNG sequence -> out/render
#   make check      naga translation report over the ISF corpus
#   make inputs     list the shader's inputs and OSC addresses
#
# Variables: SHADER, PATCH, PDFLAGS, SIZE, PORT, FRAMES, FPS, OUT
#   make run SHADER=shaders/isf-files/Angular.fs SIZE=1920x1080
#   make render SIZE=4800x7200 FRAMES=300 OUT=out/big

SHADER ?= shaders/vlfo-test.fs
PATCH  ?= pd/lfo-test.pd
PDFLAGS ?= -noaudio
PDOPEN  = -open $(PATCH)
SIZE   ?= 1280x720
PORT   ?= 9000
FRAMES ?= 90
FPS    ?= 60
OUT    ?= out/render
BIN    := target/release/vlfo
PD     ?= pd

.PHONY: all build run pd live render realtime check inputs corpus clean

all: build

build:
	cargo build --release

run: build
	$(BIN) run $(SHADER) --size $(SIZE) --port $(PORT)

pd:
	$(PD) $(PDFLAGS) -path pd $(PDOPEN)

live: build
	@$(BIN) run $(SHADER) --size $(SIZE) --port $(PORT) & V=$$!; \
	sleep 1; $(PD) $(PDFLAGS) -path pd $(PDOPEN) & P=$$!; \
	trap 'kill $$V $$P 2>/dev/null; wait $$V $$P 2>/dev/null' INT TERM EXIT; \
	while kill -0 $$V 2>/dev/null && kill -0 $$P 2>/dev/null; do sleep 0.5; done

render: build
	$(BIN) render $(SHADER) -o $(OUT) --frames $(FRAMES) --fps $(FPS) --size $(SIZE)

realtime: build
	$(BIN) render $(SHADER) -o $(OUT) --frames $(FRAMES) --fps $(FPS) --size $(SIZE) --realtime --port $(PORT)

inputs: build
	$(BIN) inputs $(SHADER)

corpus: shaders/isf-files/Angular.fs

shaders/isf-files/Angular.fs:
	./scripts/fetch-isf-files.sh

check: build corpus
	$(BIN) check shaders/isf-files/*.fs | tail -1

clean:
	cargo clean
	rm -rf out

# --- NDI ---------------------------------------------------------------------
# make serve NDI=vlfo-1                     headless 1080p60 NDI source, OSC on PORT
# make ndi4 SHADERS="a.fs b.fs c.fs d.fs"   four sources vlfo-1..4 on ports 9001..9004
# make ndi-find / make ndi-grab SOURCE=vlfo-1
NDI     ?= vlfo-1
OUTPUT  ?= 1920x1080
SHADERS ?= $(SHADER) $(SHADER) $(SHADER) $(SHADER)

.PHONY: serve ndi4 ndi-find ndi-grab

serve: build
	$(BIN) serve $(SHADER) --ndi $(NDI) --output $(OUTPUT) --fps $(FPS) --port $(PORT)

ndi4: build
	@trap 'kill 0' INT TERM EXIT; i=0; for s in $(SHADERS); do i=$$((i+1)); \
	  $(BIN) serve $$s --ndi vlfo-$$i --output $(OUTPUT) --fps $(FPS) --port $$((9000+i)) & \
	done; wait

ndi-find: build
	$(BIN) ndi-find

ndi-grab: build
	$(BIN) ndi-grab $(NDI) out/ndi-$(NDI).png
