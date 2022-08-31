# roon-extension-spotify 
*roon-extension-spotify* allows roon zones to appear as spotify connect devices. Under the hood is uses the [node-roon-api](https://github.com/RoonLabs/node-roon-api) and [librespot](https://github.com/librespot-org/librespot).
# Quickstart
TBD
# Building
### Install Node.js
Follow the instructions for installing node.js [here](https://nodejs.org/en/download/package-manager/) and verify you have both node and npm:
```
/# node
Welcome to Node.js v16.17.0.
Type ".help" for more information.
>
```
```
/# npm
npm <command>

Usage:

npm install        install all the dependencies in your project
```
### Install Rust
Follow the instructions to install [rustup](https://rustup.rs/). After this you should be able to access cargo:
```
/# cargo
Rust's package manager

USAGE:
    cargo [+toolchain] [OPTIONS] [SUBCOMMAND]
    ...
```
### Dependencies
You should be set for Mac and Windows, for Debian/Ubuntu run these commands:
```
sudo apt-get install build-essential
sudo apt-get install libasound2-dev pkg-config
```
and on Fedora this:
```
sudo dnf install gcc
sudo dnf install alsa-lib-devel
```
See [librespot](https://github.com/librespot-org/librespot/blob/master/COMPILING.md) documentation for further details.

### Source code
Clone this repository:
```
git clone git@github.com:johnnyslush/roon-extension-spotify.git
```
## Compiling and Running
cd into the node-librespot subdirectory, install all dependencies, and build the rust bindings:
```
cd node-librespot && npm i
```
Install remaining dependencies at the root directory of roon-extension-spotify:
```
cd .. && npm i
```
Run directly from node:
```
node .
```
You can also build binaries for Mac, Linux, and Windows. Execute the below command and look for the output in the dist/ directory:
```
npm run build
```
Run binary directly:
```
./dist/roon-extension-spotify-macos
```

