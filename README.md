# roon-extension-spotify 
*roon-extension-spotify* allows roon zones to appear as spotify connect devices. Under the hood it uses the [node-roon-api](https://github.com/RoonLabs/node-roon-api) and [librespot](https://github.com/librespot-org/librespot).
# Quickstart
TBD
# Building
### Install Node.js and Rust
- [node.js](https://nodejs.org/en/download/package-manager/)
- [rust](https://rustup.rs/)
### Source code
Clone this repository:
```
git clone git@github.com:johnnyslush/roon-extension-spotify.git
```
## Compiling and Running
Build the rust bindings:
```
cd node-librespot && npm i
```
Install remaining dependencies and run:
```
cd .. && npm i && node .
```
### Executables (in-development)
You can also build binaries for Mac, Linux, and Windows. Execute the below command and look for the output in the dist/ directory:
```
npm run build
```
Run binary directly:
```
./dist/roon-extension-spotify-macos
```

