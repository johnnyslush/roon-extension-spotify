const Librespot = require('.');

class Host {
    constructor(opts) {
        // Register javascript callbacks
        this.cbs = {
            ...opts.callbacks
        }
        this._ref = Librespot.init(opts.base_url, opts.listen_port, (e) => { this._SPOTIFY_EVENT(e) });
    }

    _SPOTIFY_EVENT(e) {
        const event = JSON.parse(e);
        const a = b => { console.log('UNHANDLED EVENT',b); };
        const cb = this.cbs[event.type] || a;
        cb(event);
    }
    port() {
        return Librespot.port.call(this._ref);
    }
    url() {
        return Librespot.url.call(this._ref);
    }
    async send_roon_message(msg) {
        return Librespot.send_roon_message.call(this._ref, JSON.stringify(msg));
    }
    async start() {
        while(this.doingstuff) {
            await new Promise(r => setTimeout(r, 100));
        }
        this.doingstuff = true;
        const res = await Librespot.start.call(this._ref);
        return res;
    }
    async stop() {
        while(!this.doingstuff) {
            await new Promise(r => setTimeout(r, 100));
        }
        const res = await Librespot.stop.call(this._ref);
        this.doingstuff = false;
        return res;
    }
}

module.exports = {
    Host
}
/* Helpful for debugging
const readline = require('readline').createInterface({
      input: process.stdin,
      output: process.stdout,
});
const host = new Host();
(async function() {
    try {
        await host.start();
        let count = 1;
        while (true) {
            const command = await new Promise(resolve => readline.question("Commands: new, disable [zid], playing [zid]", resolve));
            let clean = (command || '').trim();
            if (clean === 'new') {
                host.send_roon_message({
                    type: 'EnableZone',
                    name: 'TEST ' + count,
                    id:   'foobarbuzz' + count
                })
                count++;
            } else if (clean.startsWith('disable')) {
                const [_, zid] = clean.split(' ');
                host.send_roon_message({
                    type: 'DisableZone',
                    id:   'foobarbuzz' + zid
                })
            } else if (clean.startsWith('playing')) {
                const [_, zid] = clean.split(' ');
                host.send_roon_message({
                    type: 'Playing',
                    name: 'TEST ' + count,
                    id:   'foobarbuzz' + zid
                })
            } else {
                console.log('invalid command');
            }
        }
        await host.stop();
    } catch(e) {
        console.log(e);
    } finally {
        console.log('closing read interface');
        readline.close();
    }
})()


*/



