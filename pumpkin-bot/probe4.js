const { Rcon } = require('rcon-client');
(async () => {
  const r = await Rcon.connect({ host: '192.168.1.93', port: 25575, password: 'pumpkintest' });
  const cmds = [
    'list',
    'setblock 8 101 8 minecraft:stone',
    'setblock 8 101 8 minecraft:air',
    'fill 5 101 5 7 101 7 minecraft:stone',
    'execute if block 8 101 8 minecraft:air run say AIRMATCH',
  ];
  for (const c of cmds) {
    let o;
    try {
      o = await r.send(c);
    } catch (e) {
      o = 'ERR ' + e.message;
    }
    console.log(c.padEnd(42), '=>', String(o).slice(0, 110).replace(/\n/g, ' '));
  }
  r.end();
})();
