.PHONY: dist test clean
dist:
	./build.sh dist
test:
	./build.sh test
clean:
	cargo clean && rm -rf dist
