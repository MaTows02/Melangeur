# Melangeur

Le mélangeur de volume pour Windows qui descend jusqu'à l'onglet : baissez YouTube sans toucher à votre réunion Teams, le tout dans le même navigateur.

## Fonctions

- Volume et mute par application, sur tous les périphériques audio, y compris les applis du Store.
- **Volume, mute et indicateur de micro pour chaque onglet** de Brave, Chrome ou Edge, avec l'extension.
- Vumètres en direct, changement du périphérique par défaut, démarrage avec Windows.
- Très léger : environ 2 Mo de mémoire privée au démarrage et aucune activité mesurée en veille, sur la machine de développement.

## Installation

1. Téléchargez `Melangeur-Setup-1.0.0.exe` dans les [versions publiées](https://github.com/MaTows02/Melangeur/releases).
2. Lancez-le : aucun droit administrateur n'est demandé. Le programme n'étant pas encore signé, Windows SmartScreen peut afficher un avertissement (**Informations complémentaires**, puis **Exécuter quand même**).
3. Cliquez sur l'icône à trois curseurs, près de l'horloge.

Une version portable sans installation est aussi proposée.

### Volume par onglet

Windows ne connaît que le navigateur entier : régler un onglet demande une extension.

En attendant sa publication sur les stores :

1. Clic droit sur l'icône de Melangeur, **Installer l'extension (volume par onglet)**.
2. Dans la page des extensions du navigateur, activez le **Mode développeur**.
3. **Charger l'extension non empaquetée**, puis choisissez `%LOCALAPPDATA%\Melangeur\extension`.
4. Rechargez vos onglets.

## Limites connues

- Le vumètre d'un onglet suit le niveau du navigateur entier, réduit au volume de l'onglet : Windows ne fournit pas de niveau par onglet.
- Pas d'amplification au-delà de 100 %, ni de réglage du micro par onglet.
- Un son que l'extension ne peut pas intercepter ne suit pas le curseur ; le bouton muet, celui du navigateur, reste fiable.
- Firefox n'a pas encore d'extension.
- Le changement de périphérique par défaut utilise une interface Windows non documentée.

## Compilation

Prérequis : Rust stable, les Build Tools C++ de Visual Studio et, pour l'installateur, [Inno Setup 6](https://jrsoftware.org/isinfo.php).

```bash
cargo build --release
```

```bash
powershell -ExecutionPolicy Bypass -File packaging\build.ps1
```

| Dossier | Contenu |
|---|---|
| `src/` | Programme : `engine.rs` (audio), `uia.rs` (barre d'onglets), `ext.rs` (lien avec l'extension), `ui.rs` et `menu.rs` (interface) |
| `extension/` | Extension de navigateur |
| `packaging/` | Icônes, installateur, construction des livrables |
| `STORE_LISTING.md` | Textes et étapes pour publier l'extension |

## Licence

Logiciel propriétaire, code source consultable : voir [LICENSE.txt](LICENSE.txt). Données : voir [PRIVACY.md](PRIVACY.md). Composants tiers : voir [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Melangeur n'est ni affilié à EarTrumpet ni approuvé par ses auteurs.
