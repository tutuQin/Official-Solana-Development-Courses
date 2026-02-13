/** Challenge: Mint an SPL Token
 *
 * In this challenge, you will create an SPL token!
 *
 * Goal:
 *   Mint an SPL token in a single transaction using Web3.js and the SPL Token library.
 *
 * Objectives:
 *   1. Create an SPL mint account.
 *   2. Initialize the mint with 6 decimals and your public key (feePayer) as the mint and freeze authorities.
 *   3. Create an associated token account for your public key (feePayer) to hold the minted tokens.
 *   4. Mint 21,000,000 tokens to your associated token account.
 *   5. Sign and send the transaction.
 */

import {
  Keypair,
  Connection,
  sendAndConfirmTransaction,
  SystemProgram,
  Transaction,
} from "@solana/web3.js";

import {
  createAssociatedTokenAccountInstruction,
  createInitializeMint2Instruction,
  createMintToInstruction,
  createMintToCheckedInstruction,
  MINT_SIZE,
  getMinimumBalanceForRentExemptMint,
  TOKEN_PROGRAM_ID,
  getAssociatedTokenAddressSync,
  ASSOCIATED_TOKEN_PROGRAM_ID,
} from "@solana/spl-token";

import bs58 from "bs58";

// Import our keypair from the wallet file
const feePayer = Keypair.fromSecretKey(
  // ⚠️ INSECURE KEY. DO NOT USE OUTSIDE OF THIS CHALLENGE
  bs58.decode(process.env.SECRET)
);

// Create a connection to the RPC endpoint
const connection = new Connection(process.env.RPC_ENDPOINT, "confirmed");

// Entry point of your TypeScript code (we will call this)
async function main() {
  try {
    // Generate a new keypair for the mint account
    const mint = Keypair.generate();

    const mintRent = await getMinimumBalanceForRentExemptMint(connection);

    // 1. Create the mint account
    const createAccountIx = SystemProgram.createAccount({
      fromPubkey: feePayer.publicKey,
      newAccountPubkey: mint.publicKey,
      lamports: mintRent,
      space: MINT_SIZE,
      programId: TOKEN_PROGRAM_ID,
    });

    // 2. Initialize the mint account
    // Set decimals to 6, and the mint and freeze authorities to the fee payer (you).
    const initializeMintIx = createInitializeMint2Instruction(
      mint.publicKey, // mint account public key
      6, // decimals
      feePayer.publicKey, // mint authority
      feePayer.publicKey, // freeze authority
      TOKEN_PROGRAM_ID // token program ID
    );

    // 3. Create the associated token account
    const associatedTokenAccount = getAssociatedTokenAddressSync(
      mint.publicKey, // mint
      feePayer.publicKey // owner
    );

    const createAssociatedTokenAccountIx =
      createAssociatedTokenAccountInstruction(
        feePayer.publicKey, // payer
        associatedTokenAccount, // associated token account address
        feePayer.publicKey, // owner
        mint.publicKey // mint
      );

    // 4. Mint 21,000,000 tokens to the associated token account
    // Since decimals = 6, the actual amount = 21_000_000 * 10^6
    const mintAmount = 21_000_000 * 10 ** 6;
    const mintToCheckedIx = createMintToCheckedInstruction(
      mint.publicKey, // mint
      associatedTokenAccount, // destination
      feePayer.publicKey, // authority (mint authority)
      mintAmount, // amount (with decimals factored in)
      6 // decimals
    );

    const recentBlockhash = await connection.getLatestBlockhash();

    const transaction = new Transaction({
      feePayer: feePayer.publicKey,
      blockhash: recentBlockhash.blockhash,
      lastValidBlockHeight: recentBlockhash.lastValidBlockHeight,
    }).add(
      createAccountIx,
      initializeMintIx,
      createAssociatedTokenAccountIx,
      mintToCheckedIx
    );

    const transactionSignature = await sendAndConfirmTransaction(
      connection,
      transaction,
      [feePayer, mint] // feePayer signs as the payer & authority, mint signs because it's a new account being created
    );

    console.log("Mint Address:", mint.publicKey.toBase58());
    console.log("Transaction Signature:", transactionSignature);
  } catch (error) {
    console.error(`Oops, something went wrong: ${error}`);
  }
}

main();
