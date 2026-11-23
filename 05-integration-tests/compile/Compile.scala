//> using scala 2.12.18
//> using dep org.scorexfoundation::sigma-state:6.0.7

// Offline ErgoScript -> ErgoTree compiler for the five pre-genesis contracts
// (emission, treasury accumulation and governance, LP accumulation and fund).
//
// Tree format: ErgoTree v0, header 0x10, constant segregation. There is a network option.
// The network prefix only decides which address prefix PK("...") literals must
// carry: the compiler decodes each address and rejects one from another network
// ("Trying to decode mainnet address in testnet"). The decoded public key, and
// therefore the tree, does not depend on the prefix.
//
// Usage:
//   scala-cli --power run --server=false Compile.scala -- [--network=mainnet|testnet|<byte>]
//       [--print] <file.es> [NAME=value ...]
//
//   --network  address prefix PK("...") literals are decoded with (default testnet). The two multisig scripts
//              carry mainnet-encoded keys (9...), so they need --network=mainnet.
//   --print    also print the root expression (top-level ValDefs first) to stderr,
//              for checking which reads the compiler hoisted out of a branch.
//   NAME=value plain text substitution applied to the source before compiling.

import sigma.compiler.SigmaCompiler
import sigma.compiler.ir.CompiletimeIRContext
import sigma.ast._
import sigma.ast.syntax._
import org.ergoplatform.ErgoAddressEncoder
import scala.io.Source

object Compile {
  def main(args: Array[String]): Unit = {
    var network: Byte = ErgoAddressEncoder.TestnetNetworkPrefix
    var print = false
    val rest = args.filter { a =>
      if (a.startsWith("--network=")) {
        network = a.stripPrefix("--network=") match {
          case "mainnet" => ErgoAddressEncoder.MainnetNetworkPrefix
          case "testnet" => ErgoAddressEncoder.TestnetNetworkPrefix
          case n => n.toInt.toByte
        }
        false
      } else if (a == "--print") { print = true; false }
      else true
    }
    var src = Source.fromFile(rest(0)).mkString
    rest.drop(1).foreach { kv =>
      val i = kv.indexOf('=')
      src = src.replace(kv.substring(0, i), kv.substring(i + 1))
    }
    val compiler = SigmaCompiler(network)
    implicit val ir: CompiletimeIRContext = new CompiletimeIRContext
    val res = compiler.compile(Map.empty, src)
    val prop = res.buildTree.asSigmaProp
    val tree = ErgoTree.fromProposition(ErgoTree.ZeroHeader, prop)
    if (print) System.err.println(tree.toProposition(false).toString)
    println(scorex.util.encode.Base16.encode(tree.bytes))
  }
}
