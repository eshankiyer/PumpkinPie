// Two bots: attacker crits a cow, observer watches for entity_animation
// packets (id 4 = crit, 5 = magic crit) targeting the victim.
const mineflayer = require('mineflayer')
const { Rcon } = require('rcon-client')
const sleep = ms => new Promise(r => setTimeout(r, ms))
const mk = name => mineflayer.createBot({ host: '192.168.1.93', port: 25565, username: name, version: '1.21.1', auth: 'offline' })

async function main () {
  const observer = mk('Observer')
  observer.on('error', () => {})
  await new Promise(r => observer.once('login', r))
  const attacker = mk('Attacker')
  attacker.on('error', () => {})
  await new Promise(r => attacker.once('login', r))
  await sleep(4000)

  const seen = []
  for (const b of [observer, attacker]) {
    b._client.on('animation', p => {
      if (p.animation === 4 || p.animation === 5) {
        seen.push({ who: b.username, id: p.animation, entity: p.entityId })
        console.log(`${b.username} saw animation id=${p.animation} entity=${p.entityId}`)
      }
    })
  }

  const rcon = await Rcon.connect({ host: '192.168.1.93', port: 25575, password: 'pumpkintest' })
  const p = attacker.entity.position
  const [x, y, z] = [Math.floor(p.x) + 2, Math.floor(p.y), Math.floor(p.z)]
  console.log('summon:', (await rcon.send(`summon minecraft:cow ${x} ${y} ${z}`)).trim())
  await sleep(2000)

  const cow = Object.values(attacker.entities).find(e => e.name === 'cow')
  if (!cow) { console.log('RESULT: INCONCLUSIVE no cow entity visible to bot'); process.exit(1) }
  console.log('cow entityId', cow.id)

  // A crit needs the attacker falling with fall distance > 0 and not on ground.
  for (let i = 0; i < 12; i++) {
    attacker.setControlState('jump', true)
    await sleep(350)
    attacker.setControlState('jump', false)
    await sleep(180) // swing while descending
    attacker.attack(cow)
    await sleep(900)
  }
  await sleep(1500)

  const crits = seen.filter(s => s.id === 4 && s.entity === cow.id)
  console.log(`RESULT: ${crits.length > 0 ? 'PASS' : 'FAIL'} crit animations for cow: ${crits.length} (observer saw ${crits.filter(c => c.who === 'Observer').length})`)
  rcon.end(); observer.quit(); attacker.quit(); process.exit(0)
}
main().catch(e => { console.error('FATAL', e.message); process.exit(1) })
