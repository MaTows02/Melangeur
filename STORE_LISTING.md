# Publier l'extension

Fichier à envoyer : `dist/Melangeur-extension-1.0.0-store.zip`, créé par `packaging/build.ps1`.

## Étapes

1. Créer un compte développeur sur le [Chrome Web Store](https://chrome.google.com/webstore/devconsole) (frais d'inscription uniques) et sur [Edge Add-ons](https://partner.microsoft.com/dashboard/microsoftedge).
2. Envoyer le `.zip` sans publier, et noter l'**identifiant** attribué.
3. Dans `src/ext.rs`, renseigner `STORE_EXTENSION_ID` avec cet identifiant, puis reconstruire : le programme acceptera alors l'extension du store et le bouton d'installation ouvrira sa page.
4. Compléter la fiche ci-dessous, ajouter au moins une capture d'écran (1280 x 800), puis soumettre.
5. Publier une nouvelle version du programme une fois l'extension validée.

Edge attribue un identifiant différent de Chrome : si l'extension y est publiée, il faudra aussi l'ajouter à `allowed_origins`.

## Fiche

**Nom :** Melangeur : volume par onglet

**Résumé :** Réglez le volume de chaque onglet depuis le mélangeur Melangeur pour Windows.

**Description :**

> Baissez YouTube sans toucher à votre réunion. Cette extension ajoute un curseur de volume, un bouton muet et un indicateur de micro pour chaque onglet dans Melangeur, le mélangeur de volume pour Windows.
>
> Elle nécessite le programme Melangeur : https://github.com/MaTows02/Melangeur
>
> Aucune donnée n'est envoyée sur Internet. Les titres des onglets concernés sont transmis uniquement au programme Melangeur, sur votre ordinateur.

**Catégorie :** Outils

**Politique de confidentialité :** https://github.com/MaTows02/Melangeur/blob/main/PRIVACY.md (nécessite un dépôt public, sinon à héberger ailleurs)

## Justification des permissions

**Objectif unique :** régler le volume de chaque onglet depuis le programme Melangeur.

| Permission | Justification |
|---|---|
| `<all_urls>` | Le son peut venir de n'importe quel site : le réglage doit s'appliquer dans la page et dans ses sous-cadres. |
| `tabs` | Connaître les onglets qui jouent du son et afficher leur titre dans Melangeur ; couper le son et afficher un onglet à la demande. |
| `nativeMessaging` | Dialoguer avec le programme Melangeur installé sur l'ordinateur. |
| `storage` | Conserver le volume de chaque onglet pendant la session du navigateur. |
| `scripting` | Appliquer l'extension aux onglets déjà ouverts au moment de l'installation. |

**Code distant :** aucun.

**Données :** les titres de page sont utilisés pour le fonctionnement décrit, transmis uniquement au programme local. Ils ne sont ni vendus, ni transférés à des tiers, ni utilisés à d'autres fins.
