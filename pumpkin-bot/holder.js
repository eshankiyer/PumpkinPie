// Raw protocol chunk-loader. mineflayer chokes parsing Pumpkin's chunk data,
// so we drop to node-minecraft-protocol and never decode chunks: staying
// connected is all we need to keep chunks loaded and ticking.
//
// The server runs protocol 776 (MC 26.2) but node-minecraft-protocol only
// knows up to 1.21.x, so keep_alive does not round-trip and we get dropped
// with disconnect.timeout roughly every 30s. Rather than fight that, we just
// reconnect immediately, which keeps a player present near-continuously.
const mc = require('minecraft-protocol');

const NAME = process.argv[2] || 'Verifier';
let generation = 0;

function connect() {
  const gen = ++generation;
  let client;
  try {
    client = mc.createClient({
      host: '192.168.1.93',
      port: 25565,
      username: NAME,
      auth: 'offline',
      version: '1.21.1',
      skipValidation: true,
    });
  } catch (e) {
    setTimeout(connect, 1000);
    return;
  }

  let done = false;
  const again = (why) => {
    if (done) return;
    done = true;
    console.log('[' + gen + '] reconnecting after ' + why);
    try {
      client.end();
    } catch (e) {
      /* already closed */
    }
    setTimeout(connect, 500);
  };

  client.on('login', (p) => console.log('[' + gen + '] LOGIN eid=' + p.entityId));
  client.on('position', (p) => {
    try {
      client.write('teleport_confirm', { teleportId: p.teleportId });
    } catch (e) {
      /* version mismatch on the ack, harmless */
    }
    console.log('[' + gen + '] SPAWNED ' + p.x + ' ' + p.y + ' ' + p.z);
  });
  client.on('keep_alive', (p) => {
    try {
      client.write('keep_alive', { keepAliveId: p.keepAliveId });
    } catch (e) {
      /* version mismatch, we reconnect instead */
    }
  });

  client.on('kick_disconnect', () => again('kick'));
  client.on('end', () => again('end'));
  client.on('error', (e) => again('error ' + e.message));
}

connect();
