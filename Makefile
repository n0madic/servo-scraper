.PHONY: build clean update-servo

build:
	cargo build --release

clean:
	cargo clean

update-servo:
	git -C servo fetch origin
	git -C servo checkout origin/main
	git add servo
	@echo "Servo updated to $$(git -C servo rev-parse --short HEAD). Don't forget to commit."
